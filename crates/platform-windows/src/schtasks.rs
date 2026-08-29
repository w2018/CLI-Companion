//! Windows 计划任务封装（v2.2.0：daemon 看门狗）
//!
//! 仅使用当前用户级计划任务（schtasks），无需管理员权限；
//! 本模块只负责命令构造与执行，策略（是否启用）由调用方决定。

use std::process::Command;

/// 看门狗计划任务名
pub const WATCHDOG_TASK: &str = "CLICompanionWatchdog";

/// 看门狗检查间隔（分钟）
pub const INTERVAL_MINUTES: &str = "5";

/// CREATE_NO_WINDOW：schtasks 执行不闪控制台
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 创建看门狗任务的 schtasks 参数（纯函数便于测试）
///
/// `/TR` 中 exe 路径加引号，附 `--watchdog-check` 参数。
pub fn create_args(daemon_exe: &str) -> Vec<String> {
    vec![
        "/Create".into(),
        "/F".into(),
        "/TN".into(),
        WATCHDOG_TASK.into(),
        "/SC".into(),
        "MINUTE".into(),
        "/MO".into(),
        INTERVAL_MINUTES.into(),
        "/TR".into(),
        format!("\"{}\" --watchdog-check", daemon_exe),
    ]
}

/// 删除任务的参数
pub fn delete_args() -> Vec<String> {
    vec!["/Delete".into(), "/F".into(), "/TN".into(), WATCHDOG_TASK.into()]
}

/// 查询任务是否存在的参数
pub fn query_args() -> Vec<String> {
    vec!["/Query".into(), "/TN".into(), WATCHDOG_TASK.into()]
}

fn run(args: &[String]) -> std::io::Result<std::process::Output> {
    let mut cmd = Command::new("schtasks");
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output()
}

/// 看门狗任务当前是否已注册
pub fn is_enabled() -> bool {
    run(&query_args())
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 启用（注册计划任务）或停用（删除任务）
///
/// 停用时任务不存在视为成功。
pub fn set_enabled(enable: bool, daemon_exe: &str) -> Result<(), String> {
    if enable {
        let out = run(&create_args(daemon_exe)).map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!(
                "注册看门狗任务失败: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    } else {
        if !is_enabled() {
            return Ok(());
        }
        let out = run(&delete_args()).map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!(
                "删除看门狗任务失败: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 任务参数构造正确() {
        let a = create_args(r"C:\Program Files\CLI Companion\cli-companion-daemon.exe");
        assert!(a.contains(&"/Create".to_string()));
        assert!(a.contains(&"/SC".to_string()));
        assert!(a.contains(&"MINUTE".to_string()));
        assert!(a.contains(&"5".to_string()));
        let tr = a.last().unwrap();
        assert!(tr.starts_with('"') && tr.ends_with("--watchdog-check"));
        assert!(delete_args().contains(&WATCHDOG_TASK.to_string()));
        assert!(query_args().contains(&WATCHDOG_TASK.to_string()));
    }

    #[test]
    fn 默认未注册时停用为成功() {
        // 不强行清理用户机器上可能存在的任务：未注册时 set_enabled(false) 直接 Ok
        // （已注册环境下的实机验证见发布冒烟）
        if !is_enabled() {
            assert!(set_enabled(false, "dummy.exe").is_ok());
        }
    }
}
