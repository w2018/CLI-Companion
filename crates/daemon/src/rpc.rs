//! 命名管道 RPC 服务端 + 方法分发（开发文档 §6）

use crate::state::AppState;
use cli_companion_domain::{ServiceDefinition, ServiceStatus, ServicesConfig};
use cli_companion_platform::PIPE_NAME;
use cli_companion_protocol::codec;
use cli_companion_protocol::params::{InfoResult, PingResult};
use cli_companion_protocol::{error, event, method, Request, Response, RpcError};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::windows::named_pipe::ServerOptions;

/// daemon 关闭前给 GUI 响应的缓冲时间
const SHUTDOWN_GRACE_MS: u64 = 200;

/// 命名管道服务端主循环
pub async fn run_pipe_server(state: AppState) -> std::io::Result<()> {
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(PIPE_NAME)?;
    tracing::info!(pipe = PIPE_NAME, "命名管道已监听");
    loop {
        server.connect().await?;
        // 立即创建下一个实例，保证并发连接
        let client = server;
        server = ServerOptions::new().create(PIPE_NAME)?;
        let st = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(client, st).await {
                tracing::debug!("连接结束: {e}");
            }
        });
    }
}

/// 单连接处理：帧循环（请求→分发→响应）
async fn handle_connection<S>(mut stream: S, state: AppState) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let req: Request = match codec::read_frame(&mut stream).await {
            Ok(r) => r,
            Err(_) => return Ok(()), // 客户端断开或帧损坏，结束连接
        };
        let resp = handle_request(&state, req).await;
        // 写响应失败（含帧编码错误）→ 结束连接
        if let Err(e) = codec::write_frame(&mut stream, &resp).await {
            tracing::debug!("写响应失败，断开连接: {e}");
            return Ok(());
        }
    }
}

/// 分发请求并构造响应
async fn handle_request(state: &AppState, req: Request) -> Response {
    let id = req.id;
    match dispatch(state, &req).await {
        Ok(v) => Response::ok(id, v),
        Err(e) => {
            tracing::warn!(method = req.method.as_str(), "RPC 错误: {e}");
            Response::err(id, e)
        }
    }
}

/// 方法分发
async fn dispatch(state: &AppState, req: &Request) -> Result<Value, RpcError> {
    use method::Method as M;
    let params = req.params.clone().unwrap_or(Value::Null);
    match req.method {
        // ===== 系统 =====
        M::SystemPing => serde_json::to_value(PingResult {
            ok: true,
            daemon_version: env!("CARGO_PKG_VERSION").into(),
        })
        .map_err(|e| internal(e.to_string())),

        M::SystemInfo => serde_json::to_value(InfoResult {
            daemon_version: env!("CARGO_PKG_VERSION").into(),
            schema_version: cli_companion_domain::SCHEMA_VERSION,
            data_dir: state.dirs.root.display().to_string(),
            running_as_service: state.as_service,
        })
        .map_err(|e| internal(e.to_string())),

        // ===== 配置 =====
        M::ConfigGet => {
            let store = state.config.lock().await;
            Ok(json!({
                "services": store.services,
                "app": store.app,
            }))
        }

        M::ConfigUpdate => {
            // 支持部分更新：{services?: {...}, app?: {...}, webdav_password?: "..."}
            if let Some(v) = params.get("services") {
                let cfg: ServicesConfig = serde_json::from_value(v.clone()).map_err(|e| {
                    RpcError::new(
                        error::ErrorCode::Validation,
                        format!("services 配置无效: {e}"),
                    )
                })?;
                state.save_services(cfg).await.map_err(validation)?;
            }
            if let Some(v) = params.get("app") {
                let app: crate::app_config::AppConfig =
                    serde_json::from_value(v.clone()).map_err(|e| {
                        RpcError::new(error::ErrorCode::Validation, format!("app 配置无效: {e}"))
                    })?;
                state.save_app(app).await.map_err(validation)?;
            }
            // WebDAV 密码单独提交（DPAPI 加密存 secrets.json，不进 app.json）
            if let Some(pwd) = params.get("webdav_password").and_then(Value::as_str) {
                let mut store = state.config.lock().await;
                store.secrets.set_webdav_password(pwd).map_err(|e| {
                    RpcError::new(error::ErrorCode::Internal, format!("DPAPI 加密失败: {e}"))
                })?;
                let secrets = store.secrets.clone();
                drop(store);
                state.save_secrets(secrets).await.map_err(internal)?;
            }
            Ok(json!({"ok": true}))
        }

        // ===== 服务 =====
        M::ServiceList => {
            let cfg = state.services().await;
            let runtimes = state.manager.all_runtimes();
            let items: Vec<Value> = cfg
                .services
                .iter()
                .map(|svc| {
                    let rt = runtimes.get(&svc.id).cloned().unwrap_or_default();
                    json!({"service": svc, "runtime": rt})
                })
                .collect();
            Ok(json!({"services": items}))
        }

        M::ServiceCreate => {
            let svc: ServiceDefinition = parse_service(&params)?;
            // 名称唯一性
            let mut cfg = state.services().await;
            if cfg.services.iter().any(|s| s.name == svc.name) {
                return Err(RpcError::new(
                    error::ErrorCode::Conflict,
                    format!("服务名已存在: {}", svc.name),
                ));
            }
            cfg.services.push(svc.clone());
            state.save_services(cfg).await.map_err(validation)?;
            Ok(json!({"service": svc}))
        }

        M::ServiceUpdate => {
            let svc: ServiceDefinition = parse_service(&params)?;
            let mut cfg = state.services().await;
            let pos = cfg
                .services
                .iter()
                .position(|s| s.id == svc.id)
                .ok_or_else(|| {
                    RpcError::new(
                        error::ErrorCode::NotFound,
                        format!("服务不存在: {}", svc.id),
                    )
                })?;
            // 名称不与他人冲突
            if cfg
                .services
                .iter()
                .any(|s| s.id != svc.id && s.name == svc.name)
            {
                return Err(RpcError::new(
                    error::ErrorCode::Conflict,
                    format!("服务名已存在: {}", svc.name),
                ));
            }
            cfg.services[pos] = svc.clone();
            state.save_services(cfg).await.map_err(validation)?;
            Ok(json!({"service": svc}))
        }

        M::ServiceDelete => {
            let id = parse_id(&params)?;
            let cfg = state.services().await;
            if let Some(svc) = cfg.find(&id) {
                // 运行中先停止
                let _ = state.manager.stop(svc.id).await;
            }
            let mut cfg = state.services().await;
            cfg.services.retain(|s| s.id != id);
            state.save_services(cfg).await.map_err(validation)?;
            state.manager.remove_actor(&id);
            Ok(json!({"ok": true}))
        }

        M::ServiceStart => {
            let id = parse_id(&params)?;
            let cfg = state.services().await;
            let svc = cfg.find(&id).cloned().ok_or_else(|| {
                RpcError::new(error::ErrorCode::NotFound, format!("服务不存在: {id}"))
            })?;
            state
                .manager
                .start(&svc)
                .await
                .map_err(|e| RpcError::new(error::ErrorCode::ProcessSpawnFailed, e))?;
            Ok(json!({"ok": true}))
        }

        M::ServiceStop => {
            let id = parse_id(&params)?;
            state.manager.stop(id).await.map_err(validation)?;
            Ok(json!({"ok": true}))
        }

        M::ServiceRestart => {
            let id = parse_id(&params)?;
            let cfg = state.services().await;
            let svc = cfg.find(&id).cloned().ok_or_else(|| {
                RpcError::new(error::ErrorCode::NotFound, format!("服务不存在: {id}"))
            })?;
            state
                .manager
                .restart(&svc)
                .await
                .map_err(|e| RpcError::new(error::ErrorCode::ProcessSpawnFailed, e))?;
            Ok(json!({"ok": true}))
        }

        M::ServiceLogs => {
            let id = parse_id(&params)?;
            let tail = params
                .get("tail")
                .and_then(Value::as_u64)
                .unwrap_or(200)
                .min(5000) as usize;
            let path = state.dirs.service_log(&id.to_string());
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(tail);
            Ok(json!({"lines": &lines[start..], "total": lines.len()}))
        }

        // 日志清空：真正截断日志文件（服务日志轮转之外的主动清理）
        M::ServiceLogsClear => {
            let id = parse_id(&params)?;
            let path = state.dirs.service_log(&id.to_string());
            match std::fs::write(&path, "") {
                Ok(()) => Ok(json!({"ok": true})),
                // 文件不存在视为已清空
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({"ok": true})),
                Err(e) => Err(RpcError::new(
                    error::ErrorCode::Internal,
                    format!("清空日志失败: {e}"),
                )),
            }
        }

        // ===== daemon =====
        M::DaemonShutdown => {
            let stop_services = params
                .get("stop_services")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let st = state.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(SHUTDOWN_GRACE_MS)).await;
                st.shutdown(stop_services).await;
            });
            Ok(json!({"ok": true}))
        }

        // ===== 配置导入导出（阶段4实现文件对话框集成，当前返回未实现）=====
        M::ConfigImport | M::ConfigExport => Err(RpcError::new(
            error::ErrorCode::Validation,
            "配置导入/导出尚未实现，请直接操作 config/services.json",
        )),

        // ===== 同步 =====
        M::SyncStatus => state.sync.status(state).await,
        M::SyncRunNow => {
            let st = state.clone();
            state.sync.clone().run(st).await
        }
        M::SyncTest => {
            let st = state.clone();
            state.sync.test_connection(&st).await
        }
        M::SyncUnlock => Err(RpcError::new(
            error::ErrorCode::Validation,
            "V1 未启用 WebDAV LOCK，无需解锁",
        )),

        // ===== 事件 =====
        M::EventSubscribe => {
            // V1 使用轮询替代事件流；事件类型保留在协议层备用
            Ok(json!({
                "mode": "polling",
                "topics": [
                    event::EventTopic::ServiceStarted,
                    event::EventTopic::ServiceStopped,
                    event::EventTopic::ConfigChanged,
                ]
            }))
        }
    }
}

// ===== 参数解析辅助 =====

fn parse_service(params: &Value) -> Result<ServiceDefinition, RpcError> {
    let v = params
        .get("service")
        .ok_or_else(|| RpcError::new(error::ErrorCode::Validation, "缺少 service 字段"))?;
    let mut svc: ServiceDefinition = serde_json::from_value(v.clone())
        .map_err(|e| RpcError::new(error::ErrorCode::Validation, format!("服务定义无效: {e}")))?;
    // 服务端强制刷新更新时间
    svc.updated_at = chrono::Utc::now();
    Ok(svc)
}

fn parse_id(params: &Value) -> Result<uuid::Uuid, RpcError> {
    let s = params
        .get("service_id")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(error::ErrorCode::Validation, "缺少 service_id"))?;
    uuid::Uuid::parse_str(s).map_err(|e| {
        RpcError::new(
            error::ErrorCode::Validation,
            format!("service_id 无效: {e}"),
        )
    })
}

fn validation(msg: String) -> RpcError {
    RpcError::new(error::ErrorCode::Validation, msg)
}

fn internal(msg: String) -> RpcError {
    RpcError::new(error::ErrorCode::Internal, msg)
}

/// 服务状态 → 前端标签（供文档/调试使用）
#[allow(dead_code)] // 供集成测试使用
pub fn status_label(s: &ServiceStatus) -> &'static str {
    match s {
        ServiceStatus::Stopped => "stopped",
        ServiceStatus::Starting => "starting",
        ServiceStatus::Running => "running",
        ServiceStatus::Stopping => "stopping",
        ServiceStatus::Restarting => "restarting",
        ServiceStatus::Failed => "failed",
    }
}
