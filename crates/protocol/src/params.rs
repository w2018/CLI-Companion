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
}
