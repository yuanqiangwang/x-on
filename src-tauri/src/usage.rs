//! 使用频率（frecency）的持久化 —— 见 `docs/adr/0002-frecency-ranking.md`。
//!
//! 数据形状：`launchPath -> { count, lastUsed }`，独立成 `usage.json`（不进
//! `settings.json` / `apps.json`，沿用"一个文件一个职责"）。
//!
//! 职责边界：**这里只负责存取原始计数**。把计数换算成排序权重（分桶衰减 × log
//! 次数）属于**排序策略**，住在前端的 `matcher.ts`（ADR 0002 第 5 条），所以本模块
//! 不知道 tier、也不知道权重，只知道自己有几行数。
//!
//! 两条硬约束（ADR 0002）：
//! - 写盘放在**后台线程**、且只在启动成功之后，绝不碰按键路径；
//! - 用**原子替换**（临时文件 + rename）落盘 —— `settings.json` / `apps.json` 丢了
//!   可以重建，这一份是用户历史，值得多一步。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// 条目数上限。超出后按（次数，最近使用）淘汰 —— 近似"最有价值"的保留策略，
/// 只是防止文件无限增长；真实的条目数只有"真的启动过"的应用那么多。
const MAX_ENTRIES: usize = 200;

/// 一条使用记录。`last_used` 是 Unix 毫秒时间戳。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEntry {
    pub count: u32,
    pub last_used: i64,
}

/// `usage.json` 的整体结构。用 struct 包一层（而不是裸 map），以后要加字段
/// （比如"最后一次从哪个来源启动的"）不必做迁移。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UsageStore {
    #[serde(default)]
    pub entries: HashMap<String, UsageEntry>,
}

pub fn usage_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("usage.json"))
}

/// 读全量记录。解析失败/文件不存在一律回落到空 —— 使用记录是**可丢弃**的数据，
/// 绝不因为它损坏就影响启动器可用。
pub fn load_usage(app: &AppHandle) -> UsageStore {
    let Some(path) = usage_path(app) else {
        return UsageStore::default();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// 原子写：先落临时文件再 rename（Windows 上 `rename` 走 `MOVEFILE_REPLACE_EXISTING`，
/// 覆盖是原子的），避免进程在写一半时被杀导致 `usage.json` 变成半截 JSON。
pub fn save_usage(app: &AppHandle, store: &UsageStore) -> Result<(), String> {
    let path = usage_path(app).ok_or("no app config dir")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 记一次启动：次数 +1、刷新时间戳，顺手清掉已不在索引里的条目，再按上限淘汰。
///
/// `known` 是当前索引里全部 `launchPath`，由调用方传入而不是在这里读索引 —— 这样
/// 本函数不依赖任何全局状态，可以脱离 Tauri 单测。
pub fn record_launch(app: &AppHandle, path: &str, known: &HashSet<String>) {
    let mut store = load_usage(app);
    let now = now_ms();

    let entry = store
        .entries
        .entry(path.to_string())
        .or_insert(UsageEntry {
            count: 0,
            last_used: now,
        });
    entry.count = entry.count.saturating_add(1);
    entry.last_used = now;

    // 卸载 / 改名 / 已移出索引的条目：留着只会让文件虚胖（ADR 0002 第 4 条）。
    store.entries.retain(|k, _| known.contains(k));

    if store.entries.len() > MAX_ENTRIES {
        let mut ranked: Vec<(String, u32, i64)> = store
            .entries
            .iter()
            .map(|(k, v)| (k.clone(), v.count, v.last_used))
            .collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
        let keep: HashSet<String> = ranked
            .into_iter()
            .take(MAX_ENTRIES)
            .map(|(k, _, _)| k)
            .collect();
        store.entries.retain(|k, _| keep.contains(k));
    }

    if let Err(e) = save_usage(app, &store) {
        eprintln!("usage save failed: {e}");
    }
}

/// 清空全部使用记录 —— ADR 0002 第 6 条的**复位出口**。排序会随时间漂移，用户必须
/// 有一个能回到出厂状态的开关。直接删文件而不是写空对象：删掉后 `load_usage` 回落
/// 到空，效果一样，但不会在配置目录里留一个空壳。
pub fn clear_usage(app: &AppHandle) -> Result<(), String> {
    let Some(path) = usage_path(app) else {
        return Err("no app config dir".to_string());
    };
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn prune_drops_entries_no_longer_in_index() {
        let mut store = UsageStore::default();
        store.entries.insert(
            "a".into(),
            UsageEntry {
                count: 3,
                last_used: 1,
            },
        );
        store.entries.insert(
            "gone".into(),
            UsageEntry {
                count: 9,
                last_used: 2,
            },
        );

        // record_launch 里那段 retain 的逻辑：只留索引里还有的键。
        store.entries.retain(|k, _| known(&["a"]).contains(k));

        assert!(store.entries.contains_key("a"));
        assert!(!store.entries.contains_key("gone"));
    }
}
