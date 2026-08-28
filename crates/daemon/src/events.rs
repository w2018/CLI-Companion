//! 事件总线：daemon 内部状态变化 → event.subscribe 订阅连接广播
//!
//! 事件是广播机制，不能替代请求确认（开发文档 §6.2）；
//! 无订阅者时发送被静默忽略，订阅者消费不及时丢弃最旧事件。

use cli_companion_protocol::{Event, EventTopic};
use tokio::sync::broadcast;

/// 事件通道容量
const CHANNEL_CAP: usize = 256;

pub type EventTx = broadcast::Sender<Event>;

/// 创建事件总线（保留的 Sender 即总线句柄）
pub fn new_bus() -> EventTx {
    broadcast::channel(CHANNEL_CAP).0
}

/// 构造事件（时间戳取当前 UTC）
pub fn make_event(
    topic: EventTopic,
    service_id: Option<String>,
    payload: serde_json::Value,
) -> Event {
    Event::new(topic, service_id, payload, chrono::Utc::now().to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 事件构造含时间戳且可广播() {
        let tx = new_bus();
        let mut rx = tx.subscribe();
        let ev = make_event(
            EventTopic::ServiceStarted,
            Some("id-1".into()),
            json!({"name": "测试"}),
        );
        tx.send(ev.clone()).unwrap();
        let got = rx.try_recv().unwrap();
        assert_eq!(got.topic, EventTopic::ServiceStarted);
        assert!(chrono::DateTime::parse_from_rfc3339(&got.ts).is_ok());
    }

    #[test]
    fn 无订阅者发送不报错() {
        let tx = new_bus();
        let ev = make_event(EventTopic::ConfigChanged, None, json!({}));
        assert!(tx.send(ev).is_err()); // 返回 Err 表示无订阅者，属正常
    }
}
