//! 连接认证握手类型
//!
//! 阶段 0 仅定义类型；HMAC 校验逻辑在 daemon 认证模块实现（开发文档 §6.1）。

use serde::{Deserialize, Serialize};

/// daemon → GUI 的认证挑战
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthChallenge {
    /// 每连接随机 nonce（十六进制）
    pub nonce: String,
    /// 服务端时间戳（RFC 3339）
    pub ts: String,
    /// daemon 版本
    pub daemon_version: String,
}

/// GUI → daemon 的认证响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    /// HMAC-SHA256(nonce || client_pid || timestamp)，密钥为共享机器密钥
    pub hmac_hex: String,
    /// 客户端进程 ID
    pub client_pid: u32,
    /// 客户端时间戳（RFC 3339）
    pub ts: String,
    /// 客户端版本
    pub client_version: String,
}
