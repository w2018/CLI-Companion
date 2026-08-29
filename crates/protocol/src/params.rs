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
    /// CPU 占用（0-100，按逻辑核数归一化；无采样时缺省）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f32>,
    /// 进程树内存工作集（字节；无采样时缺省）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_bytes: Option<u64>,
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
        assert_eq!(r.metrics[1].cpu_percent, None);
        assert_eq!(r.metrics[1].mem_bytes, None);
    }
}
