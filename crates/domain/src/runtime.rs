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
    /// 最近一次采样的 CPU 占用（0-100，进程树聚合、按逻辑核数归一化；仅运行中服务有值）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cpu_percent: Option<f32>,
    /// 最近一次采样的进程树内存工作集（字节；仅运行中服务有值）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mem_bytes: Option<u64>,
    // ===== v2.4.0 扩展指标（全部可选，旧数据缺省兼容）=====
    /// 内存占系统物理内存百分比（仅运行中服务有值）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mem_percent: Option<f32>,
    /// GPU 利用率（0-100，进程树各引擎取最大值；无 GPU 数据时缺省）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gpu_percent: Option<f32>,
    /// 专用 GPU 内存（字节；无 GPU 数据时缺省）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gpu_mem_bytes: Option<u64>,
    /// 磁盘读速率（字节/秒，进程树聚合的逻辑 I/O）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub disk_read_bytes_per_sec: Option<u64>,
    /// 磁盘写速率（字节/秒）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub disk_write_bytes_per_sec: Option<u64>,
    /// 网络接收速率（字节/秒，TCP 口径，进程树聚合）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub net_rx_bytes_per_sec: Option<u64>,
    /// 网络发送速率（字节/秒，TCP 口径）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub net_tx_bytes_per_sec: Option<u64>,
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
            cpu_percent: None,
            mem_bytes: None,
            mem_percent: None,
            gpu_percent: None,
            gpu_mem_bytes: None,
            disk_read_bytes_per_sec: None,
            disk_write_bytes_per_sec: None,
            net_rx_bytes_per_sec: None,
            net_tx_bytes_per_sec: None,
        }
    }
}
