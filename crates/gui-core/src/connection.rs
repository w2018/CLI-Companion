//! daemon 管道客户端：每次调用按需连接（无状态，重连天然健壮）

use cli_companion_platform::PIPE_NAME;
use cli_companion_protocol::codec;
use cli_companion_protocol::{Method, Request, Response, RpcError};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

/// 全局请求 ID 计数器
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Windows ERROR_PIPE_BUSY：所有管道实例被占用（并发连接高峰），不代表 daemon 已退出
const ERROR_PIPE_BUSY: i32 = 232;

/// 并发拉起去重：多个命令同时发现 daemon 不可达时，只有一个真正执行拉起流程
static ENSURE_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 连接管道（ERROR_PIPE_BUSY 时短暂重试，总计约 1 秒）
pub(crate) async fn open_pipe() -> std::io::Result<NamedPipeClient> {
    for _ in 0..20 {
        match ClientOptions::new().open(PIPE_NAME) {
            Ok(p) => return Ok(p),
            // 实例被占用：稍等重试（服务端循环会持续补充新实例）
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::from_raw_os_error(ERROR_PIPE_BUSY))
}

pub struct DaemonConnection;

impl DaemonConnection {
    /// 下一个请求 ID（事件订阅连接复用同一计数器）
    pub fn next_id() -> u64 {
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    }

    /// 连接管道 → 发请求 → 收响应
    pub async fn call(
        method: Method,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, RpcError> {
        let mut pipe = open_pipe().await.map_err(|_| {
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

    /// 请求 daemon 关闭并等待管道消失（完全退出流程的可靠路径）。
    ///
    /// 返回 true 表示 daemon 已退出（或本就不可达）；false 表示等待超时仍存活。
    /// 不经过前端 webview，托盘兜底退出用，避免 webview 卡住时漏发关闭指令。
    pub async fn shutdown_and_wait(stop_services: bool, max_wait_ms: u64) -> bool {
        let _ = Self::call(
            Method::DaemonShutdown,
            Some(serde_json::json!({ "stop_services": stop_services })),
        )
        .await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(max_wait_ms);
        loop {
            if !Self::is_alive().await {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
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
        // 拉起流程串行化：并发命令同时发现不可达时只有一个真正拉起，
        // 其余任务在拿到守护锁后复测存活，避免误拉起多个 daemon 进程
        let _guard = ENSURE_GUARD.lock().await;
        if Self::is_alive().await {
            return Ok(true);
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
        // 4. 等待管道就绪（最多 20 秒：新 daemon 可能等待旧实例锁最长 15 秒，加配置加载）
        for _ in 0..100 {
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
