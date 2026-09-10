//! PATH 命令 —— 索引的第五类数据源。只做检索，不做带参执行（那是后续计划）。
//!
//! ## 为什么需要
//!
//! 开始菜单 `.lnk` 的英文别名来自文件 stem，所以 `chrome` 能命中「Google Chrome」。
//! 但另外两类数据源只有本地化中文名，没有任何英文 token：
//!
//! - **Store / UWP**（第四类）：AppsFolder 给的是「计算器」，于是 `calc` 搜不到；
//! - **系统功能**（第三类）：写死的是「显示」，于是 `display` 搜不到。
//!
//! 更根本的是，开始菜单压根不提供「设备管理器」「磁盘管理」这类入口——它们既没有
//! `.lnk`，也不在 AppsFolder 里，只能靠命令名到达。
//!
//! ## 收录策略：能补别名就补别名，补不上的才新增
//!
//! 这一点决定了列表里不会出现两个「计算器」：
//!
//! 1. 若前四类**已有同名条目**（按 `index_name_key` 判定），**只往它身上补英文
//!    别名**，不新增行。所以 `calc` 命中的仍是 Store 那一行——保留 UWP 图标、
//!    走 AUMID 激活，而不是另起一个 `calc.exe` 行；
//! 2. 索引里没有的（设备管理器、mstsc…）才作为新条目追加。
//!
//! ## 为什么存完整路径而不是裸命令名
//!
//! - `SHGetFileInfoW` 需要真实文件才能取到图标，裸 `devmgmt.msc` 取不到；
//! - 本机解析不到的命令**自动跳过**。Windows 家庭版没有 `gpedit` / `secpol`，
//!   专业版才有——用 PATH 解析做过滤，就不必为版本差异维护两份清单。
//!
//! ## 收录边界
//!
//! 只收 Win10/Win11 都稳定存在、且**开始菜单通常不提供**的工具。设置页已有等价
//! 入口的 `.cpl`（`timedate.cpl`、`intl.cpl`、`desk.cpl`…）一律不收——否则会和
//! `ms-settings:` 条目争同一个查询，而 Win11 里设置页才是正确的目的地。
//! 破坏性动作（关机 / 重启 / 注销）不收：启动器是「随手回车」的场景，误触代价太高。

use crate::{index_name_key, AppInfo};
use std::collections::HashMap;
use std::path::PathBuf;

/// 一条命令定义。`name` 同时承担两个角色：新条目的显示名，以及「索引里是否已有
/// 同名条目」的匹配键——所以它的取值必须和 Windows 自己的显示名一致（计算器、
/// 记事本、任务管理器…），否则补别名会补错对象或白白新增一行。
pub struct CommandDef {
    pub name: &'static str,
    /// PATH 上的命令名。带扩展名（`.msc` / `.cpl`）时按原样查找；不带时按
    /// `PATHEXT` 依次试探。
    pub cmd: &'static str,
    /// 额外的检索 token。第一个会被前端当作「原生名」显示在副标题上，所以放最
    /// 正式的英文名。
    pub aliases: &'static [&'static str],
}

/// 命令清单。分三段：多半已被前四类收录（补别名为主）／管理工具／系统工具。
pub const COMMANDS: &[CommandDef] = &[
    // —— 通常已在前四类里：命中则补别名，不新增行 ——
    CommandDef { name: "计算器", cmd: "calc", aliases: &["calculator", "calc"] },
    CommandDef { name: "记事本", cmd: "notepad", aliases: &["notepad"] },
    CommandDef { name: "画图", cmd: "mspaint", aliases: &["paint", "mspaint"] },
    CommandDef { name: "截图工具", cmd: "snippingtool", aliases: &["snippingtool", "screenshot"] },
    CommandDef { name: "Windows 终端", cmd: "wt", aliases: &["terminal", "wt"] },
    CommandDef { name: "任务管理器", cmd: "taskmgr", aliases: &["taskmgr", "task manager"] },
    CommandDef { name: "命令提示符", cmd: "cmd", aliases: &["cmd", "command prompt"] },
    CommandDef { name: "Windows PowerShell", cmd: "powershell", aliases: &["powershell", "pwsh"] },
    CommandDef { name: "注册表编辑器", cmd: "regedit", aliases: &["regedit", "registry"] },
    CommandDef { name: "文件资源管理器", cmd: "explorer", aliases: &["explorer"] },
    CommandDef { name: "控制面板", cmd: "control", aliases: &["control", "control panel"] },
    // —— 管理工具：开始菜单不提供，只能靠命令名到达 ——
    CommandDef { name: "设备管理器", cmd: "devmgmt.msc", aliases: &["devmgmt", "device manager"] },
    CommandDef { name: "磁盘管理", cmd: "diskmgmt.msc", aliases: &["diskmgmt", "disk management"] },
    CommandDef { name: "服务", cmd: "services.msc", aliases: &["services"] },
    CommandDef { name: "事件查看器", cmd: "eventvwr.msc", aliases: &["eventvwr"] },
    CommandDef { name: "计算机管理", cmd: "compmgmt.msc", aliases: &["compmgmt"] },
    CommandDef { name: "任务计划程序", cmd: "taskschd.msc", aliases: &["taskschd"] },
    CommandDef { name: "高级安全 Windows 防火墙", cmd: "wf.msc", aliases: &["wf.msc", "firewall"] },
    CommandDef { name: "证书管理器", cmd: "certmgr.msc", aliases: &["certmgr"] },
    // —— 系统工具 ——
    CommandDef { name: "系统配置", cmd: "msconfig", aliases: &["msconfig"] },
    CommandDef { name: "系统信息", cmd: "msinfo32", aliases: &["msinfo32"] },
    CommandDef { name: "资源监视器", cmd: "resmon", aliases: &["resmon"] },
    CommandDef { name: "性能监视器", cmd: "perfmon", aliases: &["perfmon"] },
    CommandDef { name: "磁盘清理", cmd: "cleanmgr", aliases: &["cleanmgr"] },
    CommandDef { name: "磁盘碎片整理", cmd: "dfrgui", aliases: &["defrag", "dfrgui"] },
    CommandDef { name: "远程桌面连接", cmd: "mstsc", aliases: &["mstsc", "rdp"] },
    CommandDef { name: "DirectX 诊断工具", cmd: "dxdiag", aliases: &["dxdiag"] },
    CommandDef { name: "音量合成器", cmd: "sndvol", aliases: &["sndvol", "volume"] },
    CommandDef { name: "字符映射表", cmd: "charmap", aliases: &["charmap"] },
    CommandDef { name: "屏幕键盘", cmd: "osk", aliases: &["osk"] },
    CommandDef { name: "放大镜", cmd: "magnify", aliases: &["magnify"] },
    CommandDef { name: "步骤记录器", cmd: "psr", aliases: &["psr"] },
    CommandDef { name: "Windows 版本信息", cmd: "winver", aliases: &["winver", "about windows"] },
    // 设置页里没有等价入口的三个 .cpl（程序和功能在 Win11 设置里叫「已安装的应用」，
    // 但卸载入口仍是 appwiz；网络连接与电源选项同理）。
    CommandDef { name: "程序和功能", cmd: "appwiz.cpl", aliases: &["appwiz", "uninstall"] },
    CommandDef { name: "系统属性", cmd: "sysdm.cpl", aliases: &["sysdm"] },
    CommandDef { name: "网络连接", cmd: "ncpa.cpl", aliases: &["ncpa"] },
    CommandDef { name: "电源选项", cmd: "powercfg.cpl", aliases: &["powercfg"] },
];

/// 新条目的来源标签。与既有标签（系统 / 商店应用 / 便携应用）同级，会显示在
/// 结果行的副标题上。
const COMMENT: &str = "系统命令";

/// 把命令清单并入索引：已存在同名条目则补别名，否则解析 PATH 后新增。
pub fn apply_commands(apps: &mut Vec<AppInfo>) {
    // 名字 → 下标。取第一个同名条目即可：前四类内部已经各自去重过了。
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for (i, a) in apps.iter().enumerate() {
        by_name.entry(index_name_key(&a.name)).or_insert(i);
    }

    for def in COMMANDS {
        let key = index_name_key(def.name);

        // 已有同名条目 → 只补英文别名，不新增行。
        if let Some(&i) = by_name.get(&key) {
            let existing = &mut apps[i];
            for alias in def.aliases {
                let s = (*alias).to_string();
                let dup = existing
                    .aliases
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(&s));
                if !dup {
                    existing.aliases.push(s);
                }
            }
            continue;
        }

        // 本机没有这条命令（例如家庭版没有 gpedit）→ 静默跳过。
        let Some(full) = resolve_on_path(def.cmd) else {
            continue;
        };
        let full = full.to_string_lossy().to_string();
        apps.push(AppInfo {
            name: def.name.to_string(),
            comment: Some(COMMENT.to_string()),
            launch_path: full.clone(),
            target_path: full,
            icon: None,
            aliases: def.aliases.iter().map(|s| (*s).to_string()).collect(),
        });
        // 登记新行，防止表内重名时插入两行。
        by_name.insert(key, apps.len() - 1);
    }
}

/// 把命令名解析成 PATH 上的完整路径；解析不到返回 `None`。
#[cfg(windows)]
pub fn resolve_on_path(cmd: &str) -> Option<PathBuf> {
    use std::path::Path;

    // 已经带路径分隔符 → 不做 PATH 查找。
    if cmd.contains('\\') || cmd.contains('/') {
        let p = PathBuf::from(cmd);
        return if p.is_file() { Some(p) } else { None };
    }

    let has_ext = Path::new(cmd).extension().is_some();
    let exts = if has_ext {
        Vec::new()
    } else {
        pathext_list()
    };

    for dir in path_dirs() {
        if has_ext {
            let candidate = dir.join(cmd);
            if candidate.is_file() {
                return Some(candidate);
            }
        } else {
            for ext in &exts {
                let candidate = dir.join(format!("{cmd}{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn resolve_on_path(_cmd: &str) -> Option<PathBuf> {
    None
}

/// `PATH` 里的目录，外加 App Execution Alias 目录。
///
/// `calc` / `wt` / `winget` 这类别名住在 `%LOCALAPPDATA%\Microsoft\WindowsApps`，
/// 它通常在 PATH 里但不保证——显式补上，否则 `wt` 会解析不到。
#[cfg(windows)]
fn path_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let aliases = PathBuf::from(local).join("Microsoft").join("WindowsApps");
        if aliases.is_dir() {
            dirs.push(aliases);
        }
    }
    dirs
}

/// `PATHEXT`（`.COM;.EXE;.BAT;.CMD;…`），仅对不带扩展名的命令使用。
#[cfg(windows)]
fn pathext_list() -> Vec<String> {
    let raw = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    raw.split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn table_has_no_duplicate_names() {
        let mut seen: HashSet<String> = HashSet::new();
        for def in COMMANDS {
            assert!(
                seen.insert(index_name_key(def.name)),
                "duplicate command name: {}",
                def.name
            );
        }
    }

    #[test]
    fn existing_entry_gets_aliases_instead_of_a_new_row() {
        // 「计算器」已由 Store 数据源提供 → 只补别名，行数不变。
        let mut apps = vec![AppInfo {
            name: "计算器".to_string(),
            comment: Some("商店应用".to_string()),
            launch_path: "store:Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".to_string(),
            target_path: "store:Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".to_string(),
            icon: None,
            aliases: Vec::new(),
        }];
        apply_commands(&mut apps);
        // 其余命令在本机解析得到，会作为新行追加——这里只关心「计算器」没有变成
        // 两行。（用 name 过滤而不是 apps.len()：总行数随本机命令数量变化。）
        let rows: Vec<_> = apps.iter().filter(|a| a.name == "计算器").collect();
        assert_eq!(rows.len(), 1, "must not add a second 计算器 row");
        assert!(rows[0].aliases.iter().any(|a| a == "calc"));
        assert!(rows[0].aliases.iter().any(|a| a == "calculator"));
        // 原条目的启动方式必须原样保留（仍走 AUMID 激活）。
        assert!(rows[0].launch_path.starts_with("store:"));
    }

    /// 诊断：清单里哪些名字会被前四类吸收（只补别名），哪些会真的新增一行。
    /// 收录策略的成败全看这个分布——如果高频工具落进了「新增」而它其实已在
    /// 索引里，说明 `name` 与 Windows 的显示名没对齐。
    #[test]
    fn report_alias_only_vs_new_rows() {
        let mut existing: HashSet<String> = HashSet::new();
        for f in crate::system::builtin_system_features() {
            existing.insert(index_name_key(f.name));
        }
        for a in crate::store::enumerate_store_apps() {
            existing.insert(index_name_key(&a.name));
        }
        for root in crate::start_menu_roots() {
            if !root.exists() {
                continue;
            }
            for e in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
                if !e.file_type().is_file() {
                    continue;
                }
                let is_lnk = e
                    .path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| x.eq_ignore_ascii_case("lnk"));
                if !is_lnk {
                    continue;
                }
                let info = crate::parse_lnk(e.path());
                if crate::drop_shortcut(&info) {
                    continue;
                }
                existing.insert(index_name_key(&info.name));
            }
        }

        let (mut aliased, mut added) = (Vec::new(), Vec::new());
        for def in COMMANDS {
            if existing.contains(&index_name_key(def.name)) {
                aliased.push(def.name);
            } else {
                added.push(def.name);
            }
        }
        eprintln!("alias-only ({}): {aliased:?}", aliased.len());
        eprintln!("new rows ({}): {added:?}", added.len());
    }

    /// 新增行走的是常规图标提取（完整路径 → `SHGetFileInfoW`）。这里确认 `.msc`、
    /// `.cpl` 这类非 `.exe` 目标也能取到图标，否则整片新增行会掉回首字母头像。
    #[test]
    fn icons_extract_for_new_rows() {
        for cmd in ["devmgmt.msc", "appwiz.cpl", "mstsc", "wt"] {
            let Some(path) = resolve_on_path(cmd) else {
                eprintln!("icon {cmd}: (command not found)");
                continue;
            };
            let label = match crate::icons::icon_data_uri(&path.to_string_lossy()) {
                Ok(Some(uri)) => format!("ok ({} bytes)", uri.len()),
                Ok(None) => "NONE".to_string(),
                Err(e) => format!("err: {e}"),
            };
            eprintln!("icon {cmd}: {label}");
        }
    }

    #[test]
    fn commands_resolve_on_this_machine() {
        // 诊断用：打印哪些命令在本机解析不到。家庭版缺 gpedit 之类属正常，
        // 但清单里的绝大多数命令应当能解析。
        let mut missing = Vec::new();
        for def in COMMANDS {
            if resolve_on_path(def.cmd).is_none() {
                missing.push(def.cmd);
            }
        }
        eprintln!(
            "xon command scan: {} of {} resolved, missing: {:?}",
            COMMANDS.len() - missing.len(),
            COMMANDS.len(),
            missing
        );
    }
}
