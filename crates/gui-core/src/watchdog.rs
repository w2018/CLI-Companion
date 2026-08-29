//! 看门狗开关的 Tauri 命令（v2.2.0）
//!
//! 计划任务以当前用户注册，无需管理员；任务执行的是 GUI 同目录的 daemon exe。

use cli_companion_platform::schtasks;

/// 查询看门狗是否已注册（阻塞调用放线程池）
#[tauri::command]
pub async fn get_watchdog_enabled() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(schtasks::is_enabled)
        .await
        .map_err(|e| e.to_string())
}

/// 启用/停用看门狗（注册或删除当前用户计划任务）
#[tauri::command]
pub async fn set_watchdog_enabled(enabled: bool) -> Result<(), String> {
    // 与 GUI 拉起 daemon 的约定一致：计划任务执行 GUI 同目录的 daemon exe
    let exe = std::env::current_exe().map_err(|e| format!("获取 GUI 路径失败: {e}"))?;
    let daemon = exe
        .parent()
        .map(|d| d.join("cli-companion-daemon.exe"))
        .ok_or_else(|| "无法定位 daemon".to_string())?;
    if !daemon.is_file() {
        return Err(format!("未找到 daemon: {}", daemon.display()));
    }
    let daemon_str = daemon.to_string_lossy().to_string();
    tauri::async_runtime::spawn_blocking(move || schtasks::set_enabled(enabled, &daemon_str))
        .await
        .map_err(|e| e.to_string())?
}
