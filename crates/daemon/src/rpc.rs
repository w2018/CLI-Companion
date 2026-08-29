//! 命名管道 RPC 服务端 + 方法分发（开发文档 §6）

use crate::state::AppState;
use cli_companion_domain::{ServiceDefinition, ServiceStatus, ServicesConfig};
use cli_companion_platform::PIPE_NAME;
use cli_companion_protocol::codec;
use cli_companion_protocol::params::{InfoResult, MetricsResult, PingResult, ServiceMetric};
use cli_companion_protocol::{error, event, method, Request, Response, RpcError};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::windows::named_pipe::ServerOptions;

/// daemon 关闭前给 GUI 响应的缓冲时间
const SHUTDOWN_GRACE_MS: u64 = 200;

/// 探测是否已有健康的 daemon 正在服务（管道 ping 可达）
///
/// 用于新实例启动时区分两种情况：
/// - 管道可达 → 已有实例在正常服务，本实例应立即退出，不留多余进程；
/// - 管道不可达（旧实例退出中）→ 等待其释放单例锁后接管。
pub async fn existing_daemon_alive() -> bool {
    use tokio::net::windows::named_pipe::ClientOptions;
    let mut pipe = match ClientOptions::new().open(PIPE_NAME) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let req = Request::new(1, method::Method::SystemPing, None);
    if codec::write_frame(&mut pipe, &req).await.is_err() {
        return false;
    }
    let read = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        codec::read_frame::<Response, _>(&mut pipe),
    )
    .await;
    matches!(read, Ok(Ok(_)))
}

/// 命名管道服务端主循环
pub async fn run_pipe_server(state: AppState) -> std::io::Result<()> {
    run_pipe_server_on(state, PIPE_NAME).await
}

/// 在指定管道名上运行服务端（集成测试用独立管道，避免与真实 daemon 冲突）
pub async fn run_pipe_server_on(state: AppState, pipe_name: &str) -> std::io::Result<()> {
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(pipe_name)?;
    tracing::info!(pipe = pipe_name, "命名管道已监听");
    loop {
        server.connect().await?;
        // 立即创建下一个实例，保证并发连接
        let client = server;
        server = ServerOptions::new().create(pipe_name)?;
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
        // 事件订阅：响应确认后进入推送长连接，直到客户端断开
        if req.method == method::Method::EventSubscribe {
            let resp = Response::ok(req.id, json!({"mode": "stream"}));
            if let Err(e) = codec::write_frame(&mut stream, &resp).await {
                tracing::debug!("订阅确认发送失败: {e}");
                return Ok(());
            }
            tracing::info!("GUI 已订阅事件流");
            return push_events(stream, state).await;
        }
        let resp = handle_request(&state, req).await;
        // 写响应失败（含帧编码错误）→ 结束连接
        if let Err(e) = codec::write_frame(&mut stream, &resp).await {
            tracing::debug!("写响应失败，断开连接: {e}");
            return Ok(());
        }
    }
}

/// 事件推送循环：广播事件逐帧下发，客户端断开或总线关闭时结束
async fn push_events<S>(mut stream: S, state: AppState) -> std::io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let mut rx = state.events.subscribe();
    loop {
        match rx.recv().await {
            Ok(ev) => {
                if let Err(e) = codec::write_frame(&mut stream, &ev).await {
                    tracing::debug!("事件推送结束（客户端断开）: {e}");
                    return Ok(());
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!("事件订阅滞后，丢弃 {n} 条旧事件");
            }
            Err(_) => return Ok(()), // 总线关闭（daemon 退出）
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
            state.emit(
                event::EventTopic::ConfigChanged,
                None,
                json!({"source": "update"}),
            );
            Ok(json!({"ok": true}))
        }

        // ===== 配置导入导出 =====
        M::ConfigExport => {
            let store = state.config.lock().await;
            // 注意：导出包含 services.json 中的环境变量值（与 WebDAV 同步范围一致），
            // WebDAV 凭据（DPAPI 加密）不导出
            Ok(json!({
                "exported_at": chrono::Utc::now().to_rfc3339(),
                "app_version": env!("CARGO_PKG_VERSION"),
                "schema_version": cli_companion_domain::SCHEMA_VERSION,
                "services": store.services,
                "app": store.app,
            }))
        }

        M::ConfigImport => {
            // 全量替换导入：{services: {...}, app?: {...}}
            let cfg: ServicesConfig = parse_config_section(&params, "services")?;
            let app: Option<crate::app_config::AppConfig> = match params.get("app") {
                Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| {
                    RpcError::new(error::ErrorCode::Validation, format!("app 配置无效: {e}"))
                })?),
                None => None,
            };
            let count = cfg.services.len();
            state.save_services(cfg).await.map_err(validation)?;
            if let Some(app) = app {
                state.save_app(app).await.map_err(validation)?;
            }
            state.emit(
                event::EventTopic::ConfigChanged,
                None,
                json!({"source": "import", "imported_services": count}),
            );
            Ok(json!({"ok": true, "imported_services": count}))
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
            state.emit(
                event::EventTopic::ConfigChanged,
                None,
                json!({"source": "create", "name": svc.name}),
            );
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
            state.emit(
                event::EventTopic::ConfigChanged,
                None,
                json!({"source": "update", "name": svc.name}),
            );
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
            state.emit(
                event::EventTopic::ConfigChanged,
                None,
                json!({"source": "delete", "service_id": id.to_string()}),
            );
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
            state.emit(
                event::EventTopic::ServiceStarted,
                Some(id.to_string()),
                json!({"name": svc.name}),
            );
            Ok(json!({"ok": true}))
        }

        M::ServiceStop => {
            let id = parse_id(&params)?;
            state.manager.stop(id).await.map_err(validation)?;
            state.emit(
                event::EventTopic::ServiceStopped,
                Some(id.to_string()),
                json!({"source": "manual"}),
            );
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
            state.emit(
                event::EventTopic::ServiceStarted,
                Some(id.to_string()),
                json!({"name": svc.name, "restart": true}),
            );
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

        // 资源指标：从 actor 运行时状态取最近一次 CPU / 内存采样
        M::ServiceMetrics => {
            let runtimes = state.manager.all_runtimes();
            let metrics = runtimes
                .into_iter()
                .map(|(id, rt)| ServiceMetric {
                    service_id: id.to_string(),
                    cpu_percent: rt.cpu_percent,
                    mem_bytes: rt.mem_bytes,
                })
                .collect();
            serde_json::to_value(MetricsResult { metrics }).map_err(|e| internal(e.to_string()))
        }

        // ===== daemon =====
        M::DaemonLogs => {
            let tail = params
                .get("tail")
                .and_then(Value::as_u64)
                .unwrap_or(200)
                .min(5000) as usize;
            let path = state.dirs.daemon_log();
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(tail);
            Ok(json!({"lines": &lines[start..], "total": lines.len()}))
        }
        // 清空 daemon.log（tracing appender 以 append 模式持有句柄，截断后继续追加到文件尾）
        M::DaemonLogsClear => {
            let path = state.dirs.daemon_log();
            match std::fs::write(&path, "") {
                Ok(()) => Ok(json!({"ok": true})),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({"ok": true})),
                Err(e) => Err(RpcError::new(
                    error::ErrorCode::Internal,
                    format!("清空 daemon 日志失败: {e}"),
                )),
            }
        }
        M::DaemonShutdown => {
            let stop_services = params
                .get("stop_services")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            state.emit(event::EventTopic::DaemonShuttingDown, None, json!({}));
            let st = state.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(SHUTDOWN_GRACE_MS)).await;
                st.shutdown(stop_services).await;
            });
            Ok(json!({"ok": true}))
        }

        // ===== 同步 =====
        M::SyncStatus => state.sync.status(state).await,
        M::SyncRunNow => {
            let st = state.clone();
            let result = state.sync.clone().run(st).await;
            state.emit(
                event::EventTopic::SyncProgress,
                None,
                json!({"source": "manual"}),
            );
            result
        }
        M::SyncTest => {
            let st = state.clone();
            state.sync.test_connection(&st).await
        }
        M::SyncUnlock => Err(RpcError::new(
            error::ErrorCode::Validation,
            "V1 未启用 WebDAV LOCK，无需解锁",
        )),

        // ===== 事件（真实推送在 handle_connection 中拦截，此分支仅为穷尽性匹配兜底）=====
        M::EventSubscribe => Ok(json!({"mode": "stream"})),
    }
}

// ===== 参数解析辅助 =====

/// 解析配置段（config.import 用）：缺失或无效均报 Validation
fn parse_config_section<T: serde::de::DeserializeOwned>(
    params: &Value,
    key: &str,
) -> Result<T, RpcError> {
    let v = params
        .get(key)
        .ok_or_else(|| RpcError::new(error::ErrorCode::Validation, format!("缺少 {key} 字段")))?;
    serde_json::from_value(v.clone())
        .map_err(|e| RpcError::new(error::ErrorCode::Validation, format!("{key} 配置无效: {e}")))
}

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
