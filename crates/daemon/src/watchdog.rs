//! daemon 看门狗（v2.2.0）
//!
//! 由计划任务每 5 分钟调用 `--watchdog-check`：daemon 可达则立即退出；
//! 不可达则静默拉起同目录的 daemon（CREATE_NO_WINDOW）后退出。
//! GUI 托管与 Win32 服务模式不依赖本机制（见 main.rs 分支）。

use crate::rpc::existing_daemon_alive;
use std::process::{Command, Stdio};

/// 看门狗检查入口：返回 true 表示 daemon 已在运行（或成功拉起）
///
/// 二次确认模式（与 GUI ensure_daemon 一致）：第一次探测失败后等 400ms
/// 再探一次，两次都不可达才拉起——避免管道瞬时繁忙被误判为"已死"。
pub async fn ensure_daemon_if_needed() -> bool {
    if existing_daemon_alive().await {
        return true;
    }
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    if existing_daemon_alive().await {
        return true;
    }
    spawn_detached().is_ok()
}

/// 静默拉起同目录 daemon（分离进程，看门狗自身退出不影响它）
pub fn spawn_detached() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(&exe);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW | DETACHED_PROCESS：无窗口且随看门狗退出继续运行
        cmd.creation_flags(0x0800_0000 | 0x0000_0008);
    }
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn 存活探测不panic() {
        // 只测探测通道本身（真正拉起 daemon 属发布冒烟验证，避免测试残留进程）
        let _ = existing_daemon_alive().await;
    }
}
