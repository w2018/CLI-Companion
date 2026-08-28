//! 事件主题与事件结构
//!
//! 事件是广播机制，不能替代请求确认（开发文档 §6.2）。

use serde::{Deserialize, Serialize};

/// 事件主题，序列化为 "service.started" 风格字符串
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventTopic {
    #[serde(rename = "service.started")]
    ServiceStarted,
    #[serde(rename = "service.stopped")]
    ServiceStopped,
    #[serde(rename = "service.health")]
    ServiceHealth,
    #[serde(rename = "service.restart_attempt")]
    ServiceRestartAttempt,
    #[serde(rename = "config.changed")]
    ConfigChanged,
    #[serde(rename = "sync.progress")]
    SyncProgress,
    #[serde(rename = "sync.conflict")]
    SyncConflict,
    #[serde(rename = "daemon.shutting_down")]
    DaemonShuttingDown,
}

/// 推送给 GUI 的事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// 事件主题
    pub topic: EventTopic,
    /// 关联服务 ID（若无则为 null）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    /// 事件载荷
    pub payload: serde_json::Value,
    /// 服务端时间戳（RFC 3339）
    pub ts: String,
}

impl Event {
    /// 构造事件（时间戳由调用方传入 RFC 3339 字符串）
    pub fn new(
        topic: EventTopic,
        service_id: Option<String>,
        payload: serde_json::Value,
        ts: String,
    ) -> Self {
        Self { topic, service_id, payload, ts }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 事件主题序列化格式() {
        let s = serde_json::to_value(EventTopic::ServiceRestartAttempt).unwrap();
        assert_eq!(s, serde_json::Value::String("service.restart_attempt".into()));
        let back: EventTopic = serde_json::from_value(s).unwrap();
        assert_eq!(back, EventTopic::ServiceRestartAttempt);
    }
}
