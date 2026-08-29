//! 失败通知：服务崩溃 / 自动重启异常时发送 Windows Toast
//!
//! - 仅 daemon 以 GUI 托管（用户会话）时发送；Win32 服务模式跑在 session 0，
//!   收不到 Toast，直接跳过（GUI 内告警仍走事件流）
//! - 用户可在设置中关闭（general.notify_on_failure）
//! - 通知发送的任何错误只记日志，绝不影响 daemon 主流程

use crate::app_config::load_app;
use crate::dirs::DataDirs;

/// Toast 使用的 AUMID：借用系统 PowerShell 的 AppUserModelID。
/// 未注册自有 AUMID 的进程无法弹 Toast，这是 Win10/11 上无安装依赖的通行做法。
const TOAST_APP_ID: &str =
    r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";

/// 服务失败类事件 → 系统 Toast
///
/// `as_service`（session 0）或用户关闭开关时静默跳过。
pub fn notify_service_failure(dirs: &DataDirs, as_service: bool, title: &str, body: &str) {
    if as_service {
        return;
    }
    if !enabled(dirs) {
        return;
    }
    send(TOAST_APP_ID, title, body);
}

/// 失败通知开关（读 app.json，通知是低频事件，直接读文件避免共享状态）
fn enabled(dirs: &DataDirs) -> bool {
    load_app(dirs).general.notify_on_failure
}

fn send(app_id: &str, title: &str, body: &str) {
    use tauri_winrt_notification::Toast;
    let result = Toast::new(app_id).title(title).text1(body).show();
    if let Err(e) = result {
        tracing::warn!("Toast 通知发送失败: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 默认开启失败通知() {
        // 临时目录没有 app.json → load_app 返回默认值（notify_on_failure=true）
        let dirs = DataDirs::resolve(Some(
            std::env::temp_dir().join(format!("cc-notify-test-{}", uuid::Uuid::new_v4())),
        ));
        assert!(enabled(&dirs));
    }

    #[test]
    fn 服务模式跳过不panic() {
        let dirs = DataDirs::resolve(Some(std::env::temp_dir()));
        // as_service=true 直接返回，不触发任何系统调用
        notify_service_failure(&dirs, true, "t", "b");
    }
}
