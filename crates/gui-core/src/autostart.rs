//! 开机自启模式管理（Windows 注册表，HKCU，无需管理员权限）
//!
//! 模式持久化：`HKCU\Software\CLICompanion` → `BootAutostartMode`
//! - `off`    不自启动
//! - `daemon` （默认）登录 Windows 后仅启动 daemon 进程，不显示 GUI 窗口
//! - `both`   登录后启动 GUI（GUI 启动会自动拉起 daemon）
//!
//! 登录启动项：`HKCU\...\CurrentVersion\Run` → `CLICompanion`，
//! 数据为带引号的可执行文件完整路径（daemon 模式指向 daemon，both 模式指向 GUI 本体）。
//! 注册表项本身即启动项的"真实状态"：删除/改写即刻生效，无需重启应用。

use std::path::PathBuf;
use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_READ, KEY_SET_VALUE};
use winreg::RegKey;

pub const MODE_OFF: &str = "off";
pub const MODE_DAEMON: &str = "daemon";
pub const MODE_BOTH: &str = "both";

const MODE_SUBKEY: &str = r"Software\CLICompanion";
const MODE_VALUE: &str = "BootAutostartMode";
const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "CLICompanion";
const DAEMON_EXE: &str = "cli-companion-daemon.exe";

/// 旧版 tauri-plugin-autostart 写入的 Run 项名（升级用户可能残留，残留会导致
/// "仅 daemon" 模式下 GUI 仍被拉起）。按候选名清理，均指向本应用，误删风险为零。
const LEGACY_RUN_VALUES: &[&str] = &[
    "CLI Companion",
    "cli-companion-gui",
    "com.cli-companion.app",
];

/// 读取当前模式；从未配置 → 默认 `daemon`（登录后自动启动 daemon）
pub fn get_mode() -> Result<String, String> {
    Ok(read_mode()?.unwrap_or_else(|| MODE_DAEMON.to_string()))
}

/// 设置模式并立即同步登录启动项
pub fn set_mode(mode: &str) -> Result<(), String> {
    let mode = match mode {
        MODE_OFF | MODE_DAEMON | MODE_BOTH => mode,
        other => return Err(format!("未知的自启动模式: {other}")),
    };
    enforce(mode)?;
    write_mode(mode)
}

/// GUI 启动时调用：
/// 1. 首次使用（无模式记录）→ 写入默认模式 `daemon`（登录后自动启动 daemon）
/// 2. 已有模式 → 把登录启动项与模式对齐（自愈：被安全软件/用户清理后补回）
///
/// 失败只记日志，绝不阻塞 GUI 启动。
pub fn apply_startup_default() {
    match read_mode() {
        Ok(Some(mode)) => {
            if let Err(e) = enforce(&mode) {
                tracing::warn!("对齐开机自启启动项失败（mode={mode}）: {e}");
            }
        }
        Ok(None) => match set_mode(MODE_DAEMON) {
            Ok(()) => tracing::info!("已应用默认开机自启：登录 Windows 后自动启动 daemon"),
            Err(e) => tracing::warn!("写入默认开机自启（daemon）失败: {e}"),
        },
        Err(e) => tracing::warn!("读取开机自启模式失败: {e}"),
    }
}

/// 把 Run 启动项对齐到指定模式（含旧插件遗留项清理）
fn enforce(mode: &str) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let run = hkcu
        .open_subkey_with_flags(RUN_SUBKEY, KEY_READ | KEY_SET_VALUE)
        .map_err(|e| format!("打开 Run 注册表项失败: {e}"))?;

    // 无论切到哪个模式，旧插件遗留项都必须清掉（否则 GUI 仍被系统拉起）
    for legacy in LEGACY_RUN_VALUES {
        match run.delete_value(legacy) {
            Ok(()) => tracing::info!("已清理旧版自启动残留项: {legacy}"),
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("清理旧版自启动项 {legacy} 失败: {e}"),
        }
    }

    if mode == MODE_OFF {
        match run.delete_value(RUN_VALUE) {
            Ok(()) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("删除登录启动项失败: {e}")),
        }
        return Ok(());
    }

    let target = if mode == MODE_DAEMON {
        let daemon = exe_dir()?.join(DAEMON_EXE);
        if !daemon.is_file() {
            return Err(format!("未找到 daemon 可执行文件：{}", daemon.display()));
        }
        daemon
    } else {
        std::env::current_exe().map_err(|e| format!("获取 GUI 路径失败: {e}"))?
    };
    // Run 项数据必须带引号：安装目录 "C:\Program Files\CLI Companion" 含空格
    let data = format!("\"{}\"", target.display());
    run.set_value(RUN_VALUE, &data)
        .map_err(|e| format!("写入登录启动项失败: {e}"))
}

fn read_mode() -> Result<Option<String>, String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(MODE_SUBKEY, KEY_READ | KEY_QUERY_VALUE)
        .map_err(|e| format!("打开注册表失败: {e}"))?;
    match key.get_value::<String, _>(MODE_VALUE) {
        Ok(v) if matches!(v.as_str(), MODE_OFF | MODE_DAEMON | MODE_BOTH) => Ok(Some(v)),
        Ok(v) => {
            tracing::warn!("注册表中的自启动模式无效（{v}），按未配置处理");
            Ok(None)
        }
        Err(_) => Ok(None),
    }
}

fn write_mode(mode: &str) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(MODE_SUBKEY)
        .map_err(|e| format!("创建注册表项失败: {e}"))?;
    key.set_value(MODE_VALUE, &mode.to_string())
        .map_err(|e| format!("写入自启动模式失败: {e}"))
}

fn exe_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("获取程序路径失败: {e}"))?;
    exe.parent()
        .map(|d| d.to_path_buf())
        .ok_or_else(|| "无法定位程序目录".into())
}
