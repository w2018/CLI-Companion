//! JSON-RPC 2.0 请求/响应 envelope

use crate::error::RpcError;
use crate::method::Method;
use serde::{Deserialize, Serialize};

/// 请求 ID；本协议使用单调递增的 u64
pub type RequestId = u64;

/// JSON-RPC 2.0 请求对象
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// 协议版本，恒为 "2.0"
    pub jsonrpc: String,
    /// 请求 ID
    pub id: RequestId,
    /// 方法名
    pub method: Method,
    /// 参数；无参数时省略
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl Request {
    /// 构造请求
    pub fn new(id: RequestId, method: Method, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            method,
            params,
        }
    }
}

/// JSON-RPC 2.0 响应对象
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// 协议版本，恒为 "2.0"
    pub jsonrpc: String,
    /// 对应请求的 ID
    pub id: RequestId,
    /// 成功结果；与 error 互斥
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// 错误对象；与 result 互斥
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    /// 成功响应
    pub fn ok(id: RequestId, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// 错误响应
    pub fn err(id: RequestId, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error),
        }
    }

    /// 取出结果或错误：成功返回 Ok(result)，失败返回 Err(RpcError)
    pub fn into_result(self) -> Result<serde_json::Value, RpcError> {
        if let Some(err) = self.error {
            Err(err)
        } else {
            Ok(self.result.unwrap_or(serde_json::Value::Null))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    #[test]
    fn 请求响应序列化往返() {
        let req = Request::new(1, Method::SystemPing, None);
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""jsonrpc":"2.0""#));
        assert!(json.contains(r#""method":"system.ping""#));
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 1);
        assert_eq!(back.method, Method::SystemPing);

        let resp = Response::ok(1, serde_json::json!({"ok": true}));
        let parsed: Response =
            serde_json::from_str(&serde_json::to_string(&resp).unwrap()).unwrap();
        assert!(parsed.into_result().is_ok());

        let err_resp = Response::err(2, RpcError::new(ErrorCode::NotFound, "无此服务"));
        let parsed: Response =
            serde_json::from_str(&serde_json::to_string(&err_resp).unwrap()).unwrap();
        let err = parsed.into_result().unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
    }
}
