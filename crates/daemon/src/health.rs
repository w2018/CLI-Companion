//! 自定义命令探活（v2.2.0，HealthKind::Command）
//!
//! 退出码 0 = 健康；非 0 / 启动失败 / 超时 = 不健康。
//! 与既有 Process 快路径互不影响：kind=Process 不经过本模块。

use std::time::Duration;

/// 单次探活超时
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// 探活结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    Healthy,
    Unhealthy(String),
}

/// 运行探活命令并判定（CREATE_NO_WINDOW，不产生闪窗）
pub async fn check_command(program: &str, args: &[String]) -> CheckOutcome {
    use tokio::process::Command;
    let mut cmd = Command::new(program);
    cmd.args(args);
    #[cfg(windows)]
    {
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return CheckOutcome::Unhealthy(format!("探活命令启动失败: {e}")),
    };
    match tokio::time::timeout(CHECK_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) if status.code() == Some(0) => CheckOutcome::Healthy,
        Ok(Ok(status)) => {
            CheckOutcome::Unhealthy(format!("探活命令退出码 {}", status.code().unwrap_or(-1)))
        }
        Ok(Err(e)) => CheckOutcome::Unhealthy(format!("探活命令执行失败: {e}")),
        Err(_) => CheckOutcome::Unhealthy(format!("探活命令超时（>{}ms）", CHECK_TIMEOUT.as_millis())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn 退出码0判定健康() {
        let r = check_command("cmd.exe", &["/C".into(), "exit 0".into()]).await;
        assert_eq!(r, CheckOutcome::Healthy);
    }

    #[tokio::test]
    async fn 退出码非0判定不健康() {
        let r = check_command("cmd.exe", &["/C".into(), "exit 3".into()]).await;
        assert!(matches!(r, CheckOutcome::Unhealthy(_)));
    }

    #[tokio::test]
    async fn 超时判定不健康() {
        // ping -n 5 约 4 秒，远超单测内改短的等待（直接用长命令 + 默认 5s 不可接受，
        // 这里用 cmd 的 choice 等待输入模拟挂起，5s 超时窗口内必不返回）
        let r = check_command("cmd.exe", &["/C".into(), "ping -n 30 127.0.0.1 > nul".into()]).await;
        assert!(matches!(r, CheckOutcome::Unhealthy(msg) if msg.contains("超时")));
    }

    #[tokio::test]
    async fn 不存在的程序判定不健康() {
        let r = check_command("no-such-binary-404.exe", &[]).await;
        assert!(matches!(r, CheckOutcome::Unhealthy(_)));
    }
}
