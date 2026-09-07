//! 内置系统功能入口：一组 hardcode 的「系统设置」URI，作为索引的固定数据源。
//!
//! 第一性原理：系统设置项 = 一个名字 + 一个可启动它的 URI，与开始菜单 `.lnk` / 便携
//! exe 本质相同，只是来源是「OS 固定资产」而非用户安装/手动加入。因此它不建子系统，
//! 只是往 `build_index` 里 append 的一个常量数组；启动走现有 `open_path`（底层
//! ShellExecuteW，已验证可打开 `ms-settings:`，见记忆 system-features-entry），图标走
//! 现有首字母头像兜底。
//!
//! 只收录 Win10/Win11 都稳定存在的高频设置页，避免版本差异导致一批失效项。

/// 一条系统功能入口：检索用的显示名 + 可被 ShellExecuteW 打开的 URI。
pub struct SystemFeature {
    pub name: &'static str,
    pub uri: &'static str,
}

/// 内置系统功能清单。URI 必须互不重复（用 uri 自身去重）。
pub fn builtin_system_features() -> &'static [SystemFeature] {
    &[
        SystemFeature { name: "个性化", uri: "ms-settings:personalization" },
        SystemFeature { name: "显示", uri: "ms-settings:display" },
        SystemFeature { name: "声音", uri: "ms-settings:sound" },
        SystemFeature { name: "网络", uri: "ms-settings:network-status" },
        SystemFeature { name: "蓝牙", uri: "ms-settings:bluetooth" },
        SystemFeature { name: "存储", uri: "ms-settings:storagesense" },
        SystemFeature { name: "关于", uri: "ms-settings:about" },
        SystemFeature { name: "默认应用", uri: "ms-settings:defaultapps" },
        SystemFeature { name: "电源和睡眠", uri: "ms-settings:powersleep" },
        SystemFeature { name: "通知", uri: "ms-settings:notifications" },
        SystemFeature { name: "锁屏", uri: "ms-settings:lockscreen" },
        SystemFeature { name: "颜色", uri: "ms-settings:colors" },
        SystemFeature { name: "任务栏", uri: "ms-settings:taskbar" },
        SystemFeature { name: "主题", uri: "ms-settings:themes" },
        SystemFeature { name: "日期和时间", uri: "ms-settings:dateandtime" },
        SystemFeature { name: "打印机和扫描仪", uri: "ms-settings:printers" },
        SystemFeature { name: "Windows 更新", uri: "ms-settings:windowsupdate" },
        SystemFeature { name: "此电脑", uri: "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}" },
        SystemFeature { name: "回收站", uri: "::{645FF040-5081-101B-9F08-00AA002F954E}" },
    ]
}
