//! RPC 错误码与错误结构

use serde::{Deserialize, Serialize};
use std::fmt;

/// 稳定错误码命名空间（开发文档 §6.2）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// 参数或 schema 不合法
    Validation,
    /// 资源不存在
    NotFound,
    /// 服务或配置正被修改
    Locked,
    /// 操作超时
    Timeout,
    /// 权限或 UAC 不满足
    PermissionDenied,
    /// 越界路径
    PathDenied,
    /// 进程启动失败（附 Win32 错误码）
    ProcessSpawnFailed,
    /// 配置修订冲突
    Conflict,
    /// 已有同步任务
    SyncBusy,
    /// daemon 忙碌
    DaemonBusy,
    /// daemon 不可达（未运行或管道断开）
    DaemonUnavailable,
    /// 方法不存在
    MethodNotFound,
    /// WebDAV 协议或 HTTP 错误
    WebdavProtocol,
    /// WebDAV 认证失败
    WebdavAuth,
    /// WebDAV 服务器错误
    WebdavServer,
    /// 内部错误（仅含稳定 message，不暴露堆栈）
    Internal,
}

/// JSON-RPC error 对象
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl RpcError {
    /// 快捷构造：带格式化消息的错误
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// 附加 data 字段
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{:?}] {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 错误码序列化为大写下划线() {
        let s = serde_json::to_value(ErrorCode::PathDenied).unwrap();
        assert_eq!(s, serde_json::Value::String("PATH_DENIED".into()));
        let back: ErrorCode = serde_json::from_value(s).unwrap();
        assert_eq!(back, ErrorCode::PathDenied);
    }
}
