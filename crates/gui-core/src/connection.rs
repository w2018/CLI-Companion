//! daemon 管道客户端：每次调用按需连接（无状态，重连天然健壮）

use cli_companion_platform::PIPE_NAME;
use cli_companion_protocol::codec;
use cli_companion_protocol::{Method, Request, Response, RpcError};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::windows::named_pipe::ClientOptions;

/// 全局请求 ID 计数器
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct DaemonConnection;

impl DaemonConnection {
    fn next_id() -> u64 {
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    }

    /// 连接管道 → 发请求 → 收响应
    pub async fn call(
        method: Method,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, RpcError> {
        let mut pipe = ClientOptions::new().open(PIPE_NAME).map_err(|_| {
            RpcError::new(
                RpcErrorCode::DaemonUnavailable,
                "守护进程不可达（未运行或管道已断开）",
            )
        })?;
        let req = Request::new(Self::next_id(), method, params);
        codec::write_frame(&mut pipe, &req).await?;
        let resp: Response = codec::read_frame(&mut pipe).await?;
        resp.into_result()
    }

    /// 轻量存活探测
    pub async fn is_alive() -> bool {
        Self::call(Method::SystemPing, None).await.is_ok()
    }

    /// 确保 daemon 在运行：先探测；未运行则从 GUI 同目录拉起（开发文档 §2.1 GUI 启动流程）
    ///
    /// 返回 true 表示 daemon 已就绪。
    pub async fn ensure_daemon() -> Result<bool, String> {
        // 1. 探测存活（二次确认：旧实例可能"还在退出中"，第一次 ping 能通但随即消亡）
        if Self::is_alive().await {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            if Self::is_alive().await {
                return Ok(true);
            }
            // 二次 ping 失败 → 旧实例正在退出，继续走拉起流程
            // （新 daemon 启动时会等待旧实例的锁释放，最多 15 秒）
        }
        // 2. 定位同目录的 daemon exe（安装版与开发版都在同一目录）
        let exe = std::env::current_exe().map_err(|e| format!("获取 GUI 路径失败: {e}"))?;
        let Some(dir) = exe.parent() else {
            return Ok(false);
        };
        let daemon = dir.join("cli-companion-daemon.exe");
        if !daemon.is_file() {
            tracing::warn!("未找到 daemon 可执行文件: {}", daemon.display());
            return Ok(false);
        }
        // 3. 拉起 daemon（CREATE_NO_WINDOW：daemon 是控制台程序，避免闪黑框）
        //    注意：GUI 退出不会连带杀死 daemon（无 Job 关联），符合"GUI 关闭服务常驻"
        let spawn_result = {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                std::process::Command::new(&daemon)
                    .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                    .spawn()
            }
            #[cfg(not(windows))]
            {
                std::process::Command::new(&daemon).spawn()
            }
        };
        if let Err(e) = spawn_result {
            return Err(format!("启动 daemon 失败: {e}"));
        }
        tracing::info!("已拉起 daemon: {}", daemon.display());
        // 4. 等待管道就绪（最多 6 秒：含单例锁、配置加载）
        for _ in 0..30 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if Self::is_alive().await {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

// 避免直接引用 protocol 内部枚举路径过长
use cli_companion_protocol::error::ErrorCode as RpcErrorCode;
