//! 长度前缀帧编解码
//!
//! 帧格式：4 字节小端长度 + JSON UTF-8 载荷。
//! 单帧上限 4 MiB，超限立即报错，防止恶意超大消息耗尽内存（开发文档 §9.3）。

use crate::error::{ErrorCode, RpcError};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// 单帧最大字节数：4 MiB
pub const MAX_FRAME_SIZE: u32 = 4 * 1024 * 1024;

/// 将任意可序列化值编码为一帧（含长度前缀）
pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let payload = serde_json::to_vec(value)?;
    let len = payload.len() as u32;
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&payload);
    Ok(buf)
}

/// 从 AsyncRead 读取一帧并反序列化
pub async fn read_frame<T: DeserializeOwned, R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<T, RpcError> {
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| RpcError::new(ErrorCode::Internal, format!("读取帧长度失败: {e}")))?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME_SIZE {
        return Err(RpcError::new(
            ErrorCode::Internal,
            format!("帧大小 {len} 超过上限 {MAX_FRAME_SIZE}"),
        ));
    }
    let mut payload = vec![0u8; len as usize];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|e| RpcError::new(ErrorCode::Internal, format!("读取帧载荷失败: {e}")))?;
    serde_json::from_slice(&payload)
        .map_err(|e| RpcError::new(ErrorCode::Internal, format!("解析帧 JSON 失败: {e}")))
}

/// 将一帧写入 AsyncWrite
pub async fn write_frame<T: Serialize, W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &T,
) -> Result<(), RpcError> {
    let buf = encode_frame(value)
        .map_err(|e| RpcError::new(ErrorCode::Internal, format!("编码帧失败: {e}")))?;
    writer
        .write_all(&buf)
        .await
        .map_err(|e| RpcError::new(ErrorCode::Internal, format!("写入帧失败: {e}")))?;
    writer
        .flush()
        .await
        .map_err(|e| RpcError::new(ErrorCode::Internal, format!("刷新写入失败: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Msg {
        text: String,
    }

    #[tokio::test]
    async fn 帧编解码往返() {
        let msg = Msg { text: "你好，CLI Companion".into() };
        let mut buf = encode_frame(&msg).unwrap();
        // 模拟流读取
        let mut cursor = std::io::Cursor::new(buf.clone());
        let decoded: Msg = read_frame(&mut cursor).await.unwrap();
        assert_eq!(decoded, msg);
        // 长度前缀 = 载荷长度
        let payload_len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        assert_eq!(payload_len as usize, buf.len() - 4);
        buf.clear();
    }

    #[tokio::test]
    async fn 超大帧被拒绝() {
        let oversized = (MAX_FRAME_SIZE + 1).to_le_bytes();
        let mut cursor = std::io::Cursor::new(oversized.to_vec());
        let err: Result<Msg, _> = read_frame(&mut cursor).await;
        assert!(err.is_err());
    }
}
