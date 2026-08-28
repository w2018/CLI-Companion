//! 运行时状态（daemon 内存中的事实，不写入 services.json）

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 服务生命周期状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    /// 重启流程中：先停止后启动
    Restarting,
    /// 上次启动/运行失败（等待退避重试或已熔断）
    Failed,
}

/// 单个服务的运行时状态快照
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeState {
    pub status: ServiceStatus,
    /// 当前或最后一次的进程 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// 累计重启次数（自 daemon 启动）
    pub restart_count: u32,
    /// 10 分钟窗口内的重启次数（熔断器计数）
    pub restarts_recent_10m: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_exit_code: Option<i32>,
    /// 最后一次健康检查结果
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_health: Option<String>,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            status: ServiceStatus::Stopped,
            pid: None,
            started_at: None,
            restart_count: 0,
            restarts_recent_10m: 0,
            last_exit_code: None,
            last_health: None,
        }
    }
}
