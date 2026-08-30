//! 各方法的请求参数与响应载荷类型
//!
//! 阶段 0 仅定义 system.* 方法；其余方法在对应阶段补充。

use serde::{Deserialize, Serialize};

/// system.ping 请求参数（空）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PingParams {}

/// system.ping 响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingResult {
    pub ok: bool,
    pub daemon_version: String,
}

/// system.info 响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfoResult {
    /// daemon 版本
    pub daemon_version: String,
    /// 协议 schema 版本
    pub schema_version: u32,
    /// 数据根目录
    pub data_dir: String,
    /// 是否以 Win32 服务运行
    pub running_as_service: bool,
}

/// service.metrics 单服务资源指标
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceMetric {
    pub service_id: String,
    /// CPU 占用（0-100，进程树聚合、按逻辑核数归一化；无采样时缺省）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f32>,
    /// 进程树内存工作集（字节；无采样时缺省）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_bytes: Option<u64>,
    // ===== v2.4.0 扩展指标（可选，旧 daemon / 旧前端双向兼容）=====
    /// 内存占系统物理内存百分比
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_percent: Option<f32>,
    /// GPU 利用率（0-100，各引擎取最大值；无 GPU 数据时缺省）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_percent: Option<f32>,
    /// 专用 GPU 内存（字节）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_mem_bytes: Option<u64>,
    /// 磁盘读速率（字节/秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_read_bytes_per_sec: Option<u64>,
    /// 磁盘写速率（字节/秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_write_bytes_per_sec: Option<u64>,
    /// 网络接收速率（字节/秒，TCP 口径）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net_rx_bytes_per_sec: Option<u64>,
    /// 网络发送速率（字节/秒，TCP 口径）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net_tx_bytes_per_sec: Option<u64>,
}

/// service.metrics 响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsResult {
    pub metrics: Vec<ServiceMetric>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping响应反序列化() {
        let v = serde_json::json!({"ok": true, "daemon_version": "0.1.0"});
        let r: PingResult = serde_json::from_value(v).unwrap();
        assert!(r.ok);
        assert_eq!(r.daemon_version, "0.1.0");
    }

    #[test]
    fn 指标响应含可选字段() {
        let v = serde_json::json!({"metrics": [
            {"service_id": "a", "cpu_percent": 12.5, "mem_bytes": 1024},
            {"service_id": "b"}
        ]});
        let r: MetricsResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.metrics.len(), 2);
        assert_eq!(r.metrics[0].cpu_percent, Some(12.5));
        assert_eq!(r.metrics[0].mem_bytes, Some(1024));
        // v2.4.0 扩展字段：旧载荷缺省为 None
        assert_eq!(r.metrics[0].gpu_percent, None);
        assert_eq!(r.metrics[0].net_rx_bytes_per_sec, None);
        assert_eq!(r.metrics[1].cpu_percent, None);
        assert_eq!(r.metrics[1].mem_bytes, None);
    }

    #[test]
    fn 指标序列化缺省字段不输出() {
        // 全 None 序列化后仅含 service_id：旧前端解析不受影响
        let m = ServiceMetric {
            service_id: "a".into(),
            cpu_percent: None,
            mem_bytes: None,
            mem_percent: None,
            gpu_percent: None,
            gpu_mem_bytes: None,
            disk_read_bytes_per_sec: None,
            disk_write_bytes_per_sec: None,
            net_rx_bytes_per_sec: None,
            net_tx_bytes_per_sec: None,
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v, serde_json::json!({"service_id": "a"}));
        // 有值的扩展字段正常往返（33.0 为 f32 可精确表示值）
        let m2 = ServiceMetric {
            gpu_percent: Some(33.0),
            gpu_mem_bytes: Some(1024 * 1024),
            net_tx_bytes_per_sec: Some(2048),
            ..m
        };
        let v = serde_json::to_value(&m2).unwrap();
        assert_eq!(v["service_id"], serde_json::json!("a"));
        assert_eq!(v["gpu_percent"], serde_json::json!(33.0));
        assert_eq!(v["net_tx_bytes_per_sec"], serde_json::json!(2048));
        let back: ServiceMetric = serde_json::from_value(v).unwrap();
        assert_eq!(back.gpu_percent, Some(33.0));
        assert_eq!(back.net_tx_bytes_per_sec, Some(2048));
    }
}
