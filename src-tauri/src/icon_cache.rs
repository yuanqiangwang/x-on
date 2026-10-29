//! 图标磁盘缓存。
//!
//! ## 为什么需要
//!
//! `SHGetFileInfoW` + GDI + PNG 编码 + base64 是**进程级**的一次性成本：`IconCache`
//! 的内存 map 只活在当前进程里，所以每次开机后的第一次唤起都要重付一遍。本机实测
//! 首屏 6 枚图标冷提取 86 ms（首枚 51 ms 是 shell 图标缓存初始化），加上 `GetDIBits`
//! / PNG / base64 / 一次 IPC 往返 / 回包后的整表重绘，用户看到的是约 120 ms 的
//! 「字母头像 → 图标」空窗。
//!
//! 把结果落到 `app_config_dir()/icon-cache.json`，下次启动时直接读回来 —— 本机索引
//! 约 120 条、单枚 data URI 2–4 KB，整份文件 300–500 KB，读 + 解析约 5 ms。
//!
//! ## 首帧就有图标
//!
//! 光有磁盘还不够：如果仍然等前端发 `get_app_icons`，那还是「先字母、后图标、再整表
//! 重绘」。所以 `attach()` 在**索引构建时**就把命中的图标挂到 `AppInfo.icon` 上，
//! 随 `scan_apps` 一起下发 —— 前端首帧渲染就有图标，零 IPC、零重绘。
//!
//! 只有「磁盘里没有」的条目（首次运行、新装应用）才走原来的异步提取路径。
//!
//! ## 失效判据：`launchPath` 的 mtime + size
//!
//! ⚠️ 刻意**不**把 `targetPath` 的 stat 并进来。`.lnk` 的图标实际来自它指向的 exe，
//! 所以「exe 原地升级、`.lnk` 不动」时图标会陈旧 —— 但把 target 纳入判据要求
//! `build_index` 和 `get_app_icons` 两处算出**完全一致**的 stamp，而后者只拿得到
//! `launchPath`（前端传回来的就是路径字符串）。两处算法一旦漂移，就会退化成
//! 「永远 miss、每次启动重新提取」，比图标陈旧糟得多。
//!
//! 这个取舍的兜底是托盘里的「清除图标缓存」。`.lnk` 被重建 / 改目标时 mtime 会变，
//! 那条路径仍然自动失效。
//!
//! ## 并发
//!
//! 三个锁各管一层：`map` 是进程内的提取结果（含「提取成功但没图标」的 `None`，
//! 前端靠它省掉重复 IPC），`disk` 是落盘内容的镜像，`loaded` 是「磁盘读过没有」的
//! 一次性闸门 —— 索引扫描和首次 IPC 都可能先到，两边都要过这道门。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::{store, AppInfo};

const CACHE_FILE: &str = "icon-cache.json";
const TEMP_FILE: &str = "icon-cache.json.tmp";

/// 缓存格式版本。改提取管线（图标尺寸、编码方式、判据算法）时必须 +1，
/// 否则旧文件会被当成有效数据读回来。
const FORMAT_VERSION: u32 = 1;

/// 落盘节流。一批提取完成后会调 `flush()`，但 500 ms 内只真正写一次 —— 快速敲键
/// 每轮可能只新增一两条，不值得每轮重写 300 KB。漏掉的最后一次由下一次
/// `get_app_icons` 补上；进程若在此之前退出，那几条下次重新提取，无副作用。
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    /// 源文件的 `mtime_nanos|size`。空串 = 没有可 stat 的源（store / 系统 URI）。
    stamp: String,
    /// 完整 data URI。存原文而非 PNG 字节，省掉启动时再 base64 一遍
    /// （120 条 × 0.1 ms 不值得省那 25% 磁盘）。
    uri: String,
}

#[derive(Serialize, Deserialize)]
struct FileFormat {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: HashMap<String, Entry>,
}

#[derive(Default)]
struct Disk {
    entries: HashMap<String, Entry>,
    dirty: bool,
    last_flush: Option<Instant>,
}

/// 图标缓存（内存 + 磁盘）。`Clone` 只为能 move 进 `spawn_blocking`，三个字段都是
/// `Arc`，克隆共享同一份状态。
#[derive(Clone, Default)]
pub struct IconCache {
    map: Arc<Mutex<HashMap<String, Option<String>>>>,
    disk: Arc<Mutex<Disk>>,
    loaded: Arc<AtomicBool>,
}

impl IconCache {
    // -----------------------------------------------------------------------
    // 索引侧
    // -----------------------------------------------------------------------

    /// 把磁盘缓存里仍然新鲜的图标挂到索引行上。索引构建时调用 —— 这是「首帧就有
    /// 图标」的唯一来源。
    ///
    /// 顺带按本次索引裁剪缓存：索引里已经不存在的条目（卸载、便携 exe 被删）留着
    /// 只会让文件无限增长。缓存是索引的附属，索引说了算。
    pub fn attach(&self, app: &AppHandle, apps: &mut [AppInfo]) {
        self.ensure_loaded(app);

        let keep: HashSet<String> = apps.iter().map(|a| key(&a.launch_path)).collect();
        {
            let mut disk = self.disk.lock().unwrap();

            for a in apps.iter_mut() {
                // 系统项走 emoji 头像，前端根本不会为它们取图标。
                if a.icon.is_some() || crate::is_system_uri(&a.launch_path) {
                    continue;
                }
                if let Some(uri) = pick(&disk.entries, &a.launch_path) {
                    a.icon = Some(uri);
                }
            }

            // ⚠️ 空索引（扫描异常）时 `keep` 为空，直接裁会把整份缓存清光 ——
            // 宁可留一堆用不上的条目，也不要因为一次失败的扫描把用户攒的图标全丢掉。
            if !keep.is_empty() {
                let before = disk.entries.len();
                disk.entries.retain(|k, _| keep.contains(k));
                if disk.entries.len() != before {
                    disk.dirty = true;
                }
            }
        }

        // 裁剪结果要落盘，否则文件会带着已卸载应用的条目一直长下去。
        self.flush(app);
    }

    // -----------------------------------------------------------------------
    // 提取侧（get_app_icons 用）
    // -----------------------------------------------------------------------

    /// 内存里已有的结果。`None` 表示「提取过了但没有图标」，与「没查过」不同 ——
    /// 前端拿它区分「该重试」和「放弃」。
    pub fn memory_hits(
        &self,
        paths: &[String],
    ) -> (HashMap<String, Option<String>>, Vec<String>) {
        let mut hits = HashMap::new();
        let mut missing = Vec::new();
        let guard = self.map.lock().unwrap();
        for p in paths {
            match guard.get(p) {
                Some(v) => {
                    hits.insert(p.clone(), v.clone());
                }
                None => missing.push(p.clone()),
            }
        }
        (hits, missing)
    }

    /// 磁盘命中。stamp 不一致（文件变过）算 miss —— 调用方会重新提取并覆盖。
    pub fn lookup_disk(&self, path: &str) -> Option<String> {
        let disk = self.disk.lock().unwrap();
        pick(&disk.entries, path)
    }

    /// 把这一轮的提取结果写进内存 + 磁盘。
    ///
    /// ⚠️ 只有 `Ok(_)` 的结果才会到这里（调用方过滤过）—— 提取失败（`Err`）必须
    /// 保持不缓存，否则前端那套 `MAX_ICON_TRIES` 重试预算就废了。
    pub fn remember(&self, got: &HashMap<String, Option<String>>) {
        if let Ok(mut map) = self.map.lock() {
            for (k, v) in got {
                map.insert(k.clone(), v.clone());
            }
        }
        let mut disk = self.disk.lock().unwrap();
        for (k, v) in got {
            if let Some(uri) = v {
                disk.entries.insert(
                    key(k),
                    Entry {
                        stamp: stamp(k),
                        uri: uri.clone(),
                    },
                );
                disk.dirty = true;
            }
        }
    }

    /// 按节流写盘。
    pub fn flush(&self, app: &AppHandle) {
        let text = {
            let mut disk = self.disk.lock().unwrap();
            if !disk.dirty {
                return;
            }
            if let Some(t) = disk.last_flush {
                if t.elapsed() < FLUSH_INTERVAL {
                    return;
                }
            }
            let Ok(text) = serde_json::to_string(&FileFormat {
                version: FORMAT_VERSION,
                entries: disk.entries.clone(),
            }) else {
                return;
            };
            disk.dirty = false;
            disk.last_flush = Some(Instant::now());
            text
        };
        write_file(app, &text);
    }

    // -----------------------------------------------------------------------
    // 托盘
    // -----------------------------------------------------------------------

    /// 清空内存 + 磁盘缓存，并立即把空文件落盘（否则下次启动会把旧文件读回来）。
    /// 调用方随后要重建索引，让不带图标的快照替换掉当前这份。
    pub fn clear(&self, app: &AppHandle) {
        if let Ok(mut map) = self.map.lock() {
            map.clear();
        }
        {
            let mut disk = self.disk.lock().unwrap();
            disk.entries.clear();
            disk.dirty = false;
            disk.last_flush = Some(Instant::now());
        }
        let text = serde_json::to_string(&FileFormat {
            version: FORMAT_VERSION,
            entries: HashMap::new(),
        })
        .unwrap_or_default();
        write_file(app, &text);
    }

    // -----------------------------------------------------------------------

    /// 首次访问时把磁盘缓存读进内存。索引扫描和首次 IPC 都可能先到，两边都要过
    /// 这道门，所以是「检查 → 加锁 → 再检查」。
    pub fn ensure_loaded(&self, app: &AppHandle) {
        if self.loaded.load(Ordering::Acquire) {
            return;
        }
        let mut disk = self.disk.lock().unwrap();
        if self.loaded.load(Ordering::Acquire) {
            return;
        }
        disk.entries = read_file(app);
        self.loaded.store(true, Ordering::Release);
    }
}

/// 缓存键：Windows 路径大小写不敏感，统一小写，免得 `C:\` 与 `c:\` 各存一份。
fn key(launch_path: &str) -> String {
    launch_path.to_lowercase()
}

/// 从缓存表里取一条**仍然新鲜**的图标。`stamp` 不一致（源文件变过）算 miss。
fn pick(entries: &HashMap<String, Entry>, launch_path: &str) -> Option<String> {
    let entry = entries.get(&key(launch_path))?;
    (entry.stamp == stamp(launch_path)).then(|| entry.uri.clone())
}

/// 失效指纹。
///
/// - store / 系统 URI：没有文件可 stat，只能靠 `FORMAT_VERSION` 整体失效；
/// - 磁盘上的文件（`.lnk` / `.exe`）：`mtime + size`。
fn stamp(launch_path: &str) -> String {
    if store::is_store_launch_path(launch_path) || crate::is_system_uri(launch_path) {
        return String::new();
    }
    file_stamp(Path::new(launch_path))
}

fn file_stamp(path: &Path) -> String {
    use std::time::UNIX_EPOCH;

    let Ok(meta) = std::fs::metadata(path) else {
        // 与「本来就没有源文件」区分开：文件读不到时返回 "-"，否则一条被删掉的
        // `.lnk` 会跟 store 条目撞成同一个空 stamp。
        return "-".to_string();
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{mtime}|{}", meta.len())
}

fn read_file(app: &AppHandle) -> HashMap<String, Entry> {
    let Ok(dir) = app.path().app_config_dir() else {
        return HashMap::new();
    };
    read_dir(&dir)
}

/// 真正的读取逻辑，与 `AppHandle` 解耦（测试要能喂一个临时目录）。
fn read_dir(dir: &Path) -> HashMap<String, Entry> {
    let Ok(text) = std::fs::read_to_string(dir.join(CACHE_FILE)) else {
        return HashMap::new();
    };
    let Ok(file) = serde_json::from_str::<FileFormat>(&text) else {
        // 文件损坏（半写、被手改）就当没有 —— 缓存丢了只会重新提取，不值得报错。
        return HashMap::new();
    };
    if file.version != FORMAT_VERSION {
        return HashMap::new();
    }
    file.entries
}

/// 原子落盘：先写临时文件再 rename。
///
/// Windows 上 `std::fs::rename` 走 `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`，可以直接
/// 覆盖已存在的目标 —— 所以中途崩溃不会留下半写的 `icon-cache.json`。
fn write_file(app: &AppHandle, text: &str) {
    let Ok(dir) = app.path().app_config_dir() else {
        return;
    };
    write_dir(&dir, text);
}

fn write_dir(dir: &Path, text: &str) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = dir.join(TEMP_FILE);
    if std::fs::write(&tmp, text).is_err() {
        return;
    }
    if std::fs::rename(&tmp, dir.join(CACHE_FILE)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::path::PathBuf;

    /// 每个测试一个独立临时目录；不用 `tempfile` 依赖，省得为一个测试拉一个新 crate。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xon-iconcache-test-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, bytes: &[u8]) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(bytes).unwrap();
        f.sync_all().unwrap();
    }

    fn entry(stamp: &str, uri: &str) -> Entry {
        Entry { stamp: stamp.to_string(), uri: uri.to_string() }
    }

    #[test]
    fn key_is_case_insensitive() {
        assert_eq!(key("C:\\Users\\A\\App.LNK"), key("c:\\users\\a\\app.lnk"));
    }

    /// store / 系统 URI 没有可 stat 的源 → 空 stamp（只靠 FORMAT_VERSION 整体失效）。
    #[test]
    fn stamp_of_non_file_launch_paths_is_empty() {
        assert_eq!(stamp("store:Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), "");
        assert_eq!(stamp("ms-settings:display"), "");
        assert_eq!(stamp("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"), "");
    }

    /// 文件不存在必须与「本来就没有源」区分开，否则一条被删掉的 `.lnk` 会跟 store
    /// 条目撞成同一个空 stamp。
    #[test]
    fn stamp_of_missing_file_is_dash() {
        let missing = std::env::temp_dir().join("xon-definitely-not-here-9f3a.lnk");
        assert_eq!(stamp(&missing.to_string_lossy()), "-");
    }

    /// stamp 跟着文件内容 / 大小走：内容变了 stamp 就得变，否则缓存永远不会失效。
    #[test]
    fn stamp_changes_when_the_source_file_changes() {
        let dir = temp_dir("stamp");
        let file = dir.join("a.exe");
        write(&file, b"one");
        let p = file.to_string_lossy().to_string();
        let first = stamp(&p);
        assert_ne!(first, "-");

        write(&file, b"a-much-longer-payload");
        assert_ne!(stamp(&p), first, "文件变了 stamp 必须跟着变");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pick_requires_a_matching_stamp() {
        let dir = temp_dir("pick");
        let file = dir.join("b.exe");
        write(&file, b"payload");
        let p = file.to_string_lossy().to_string();
        let fresh = stamp(&p);

        let mut entries = HashMap::new();
        entries.insert(key(&p), entry(&fresh, "data:image/png;base64,FRESH"));
        assert_eq!(pick(&entries, &p).as_deref(), Some("data:image/png;base64,FRESH"));

        // 大小写不同的同一个路径也要命中。
        assert!(pick(&entries, &p.to_uppercase()).is_some());

        // stamp 过期 → miss（调用方会重新提取并覆盖）。
        entries.insert(key(&p), entry("0|0", "data:image/png;base64,STALE"));
        assert_eq!(pick(&entries, &p), None);

        // 根本没收录 → miss。
        assert_eq!(pick(&entries, "C:\\nope\\nope.exe"), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 写 → 读往返，且写盘是「临时文件 + rename」，不留 `.tmp`。
    #[test]
    fn file_roundtrip() {
        let dir = temp_dir("roundtrip");
        let mut entries = HashMap::new();
        entries.insert(
            "c:\\a\\b.lnk".to_string(),
            entry("123|456", "data:image/png;base64,QUJD"),
        );
        let text = serde_json::to_string(&FileFormat { version: FORMAT_VERSION, entries }).unwrap();
        write_dir(&dir, &text);

        let back = read_dir(&dir);
        assert_eq!(back.len(), 1);
        let e = back.get("c:\\a\\b.lnk").expect("key 应原样保留");
        assert_eq!(e.stamp, "123|456");
        assert_eq!(e.uri, "data:image/png;base64,QUJD");
        assert!(!dir.join(TEMP_FILE).exists(), "临时文件必须已被 rename 掉");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 版本号对不上就整份丢弃 —— 这是改了提取管线之后唯一的自动失效手段。
    #[test]
    fn version_mismatch_discards_everything() {
        let dir = temp_dir("version");
        let text = r#"{"version":0,"entries":{"a":{"stamp":"","uri":"x"}}}"#;
        write_dir(&dir, text);
        assert!(read_dir(&dir).is_empty());

        // 损坏的 JSON 同理：当作没有，不 panic。
        write_dir(&dir, "{not json");
        assert!(read_dir(&dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
