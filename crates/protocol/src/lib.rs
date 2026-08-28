//! CLI Companion 本地 RPC 协议
//!
//! 定义 GUI 与 daemon 之间的 JSON-RPC 2.0 协议：
//! - 方法枚举（[`Method`]）
//! - 错误码（[`ErrorCode`]、[`RpcError`]）
//! - 请求/响应 envelope（[`Request`]、[`Response`]）
//! - 事件主题与载荷（[`Event`]、[`EventTopic`]）
//! - 认证握手类型（[`auth`]）
//! - 长度前缀帧编解码（[`codec`]）

pub mod auth;
pub mod codec;
pub mod envelope;
pub mod error;
pub mod event;
pub mod method;
pub mod params;

pub use envelope::{Request, Response};
pub use error::{ErrorCode, RpcError};
pub use event::{Event, EventTopic};
pub use method::Method;
