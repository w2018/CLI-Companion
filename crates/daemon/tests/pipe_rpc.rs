//! 集成测试：命名管道 RPC 全链路 + 服务生命周期
//!
//! 覆盖：system.ping / service.create / start / list / stop / delete / config.get

use std::os::windows::process::CommandExt;

use cli_companion_daemon::state::AppState;
use cli_companion_domain::{ConsoleMode, ServiceDefinition};
use cli_companion_protocol::codec;
use cli_companion_protocol::method::Method;
use cli_companion_protocol::{Request, Response};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::windows::named_pipe::ClientOptions;
use tokio::sync::Mutex as AsyncMutex;

// ===== 测试辅助 =====

/// 测试专用管道名（每测试进程唯一，避免与真实 daemon 或其他进程冲突）
fn test_pipe() -> &'static str {
    static PIPE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PIPE.get_or_init(|| format!(r"\\.\pipe\cli-companion-test-{}", std::process::id()))
}

/// 构造独立 AppState（临时目录，不影响真实配置）
fn test_state(tag: &str) -> AppState {
    let dirs = cli_companion_daemon::dirs::DataDirs::resolve(Some(
        std::env::temp_dir().join(format!("cli-comp-it-{}-{}", tag, std::process::id())),
    ));
    AppState {
        as_service: false,
        manager: Arc::new(cli_companion_daemon::manager::ServiceManager::new(
            dirs.clone(),
            Arc::new(cli_companion_daemon::events::new_bus()),
            false,
        )),
        config: Arc::new(AsyncMutex::new(cli_companion_daemon::state::ConfigStore {
            services: cli_companion_domain::ServicesConfig::default(),
            app: cli_companion_daemon::app_config::AppConfig::default(),
            secrets: Default::default(),
        })),
        sync: Arc::new(cli_companion_daemon::sync::SyncEngine::new()),
        events: Arc::new(cli_companion_daemon::events::new_bus()),
        shutdown: Arc::new(tokio::sync::Notify::new()),
        dirs,
    }
}

/// 测试专用 RPC 客户端：连接管道 → 请求 → 响应
async fn rpc_call(
    method: Method,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, cli_companion_protocol::RpcError> {
    use cli_companion_protocol::error::ErrorCode;
    let mut pipe = ClientOptions::new().open(test_pipe()).map_err(|_| {
        cli_companion_protocol::RpcError::new(ErrorCode::DaemonUnavailable, "daemon 不可达")
    })?;
    static ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(10000);
    let req = Request::new(
        ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        method,
        params,
    );
    codec::write_frame(&mut pipe, &req).await?;
    let resp: Response = codec::read_frame(&mut pipe).await?;
    resp.into_result()
}

/// 等待 RPC 服务就绪（必须真正 ping 成功：
/// 仅 open 成功可能消耗掉服务端 pending 实例，下一请求会竞态失败）
async fn wait_pipe_ready() {
    for _ in 0..50 {
        if rpc_call(Method::SystemPing, None).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("ping 100ms×50 后仍不可用");
}

/// 生成一个长驻测试服务定义（ping 本地回环，无控制台窗口）
fn sleeper_service(name: &str, seconds: u32) -> ServiceDefinition {
    let mut svc = ServiceDefinition::new(
        name,
        std::path::PathBuf::from("C:\\Windows\\System32\\PING.EXE"),
    );
    svc.args = vec![
        cli_companion_domain::Arg {
            id: "a1".into(),
            key: "-n".into(),
            value: Some(seconds.to_string()),
            enabled: true,
            kind: cli_companion_domain::ArgKind::Option,
            description: String::new(),
        },
        cli_companion_domain::Arg {
            id: "a2".into(),
            key: String::new(),
            value: Some("127.0.0.1".into()),
            enabled: true,
            kind: cli_companion_domain::ArgKind::Positional,
            description: String::new(),
        },
    ];
    svc.console.mode = ConsoleMode::NoConsole; // 测试不弹窗口
    svc
}

// ===== 测试用例 =====

/// 每个用例独立连接：管道名全局唯一，测试串行执行（一个进程一个管道实例集）
/// 串联所有用例于单个 #[tokio::test]，避免管道名冲突
#[tokio::test(flavor = "multi_thread")]
async fn 管道rpc全链路与服务生命周期() {
    let state = test_state("rpc");
    // 启动 RPC 服务端（独立测试管道，避免与正在运行的真实 daemon 冲突）
    {
        let st = state.clone();
        tokio::spawn(async move {
            let _ = cli_companion_daemon::rpc::run_pipe_server_on(st, test_pipe()).await;
        });
    }
    wait_pipe_ready().await;

    // ===== 1. system.ping =====
    let pong = rpc_call(Method::SystemPing, None)
        .await
        .expect("ping 应成功");
    assert_eq!(pong["ok"], json!(true));
    assert!(pong["daemon_version"].as_str().is_some());

    // ===== 2. system.info =====
    let info = rpc_call(Method::SystemInfo, None)
        .await
        .expect("info 应成功");
    assert_eq!(info["running_as_service"], json!(false));

    // ===== 3. service.create =====
    let svc = sleeper_service("集成测试服务", 60);
    let created = rpc_call(
        Method::ServiceCreate,
        Some(json!({ "service": svc.clone() })),
    )
    .await
    .expect("创建服务应成功");
    let sid = created["service"]["id"].as_str().unwrap().to_string();

    // ===== 4. service.start =====
    rpc_call(Method::ServiceStart, Some(json!({ "service_id": sid })))
        .await
        .expect("启动服务应成功");

    // 轮询等待进入 Running
    let mut running = false;
    for _ in 0..50 {
        let list = rpc_call(Method::ServiceList, None).await.unwrap();
        if list["services"][0]["runtime"]["status"] == json!("running") {
            running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(running, "服务应进入 running 状态");
    let list = rpc_call(Method::ServiceList, None).await.unwrap();
    let pid = list["services"][0]["runtime"]["pid"]
        .as_u64()
        .expect("应有 pid");

    // ===== 5. service.logs（ping 会输出，应有内容）=====
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let logs = rpc_call(
        Method::ServiceLogs,
        Some(json!({ "service_id": sid, "tail": 10 })),
    )
    .await
    .unwrap();
    assert!(logs["total"].as_u64().unwrap() > 0, "日志应有内容");

    // ===== 6. service.stop =====
    rpc_call(Method::ServiceStop, Some(json!({ "service_id": sid })))
        .await
        .expect("停止服务应成功");
    let mut stopped = false;
    for _ in 0..50 {
        let list = rpc_call(Method::ServiceList, None).await.unwrap();
        if list["services"][0]["runtime"]["status"] == json!("stopped") {
            stopped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(stopped, "服务应进入 stopped 状态");
    // 进程确实不存在了
    assert!(!process_alive(pid as u32), "停止后进程应不存在");

    // ===== 7. service.delete =====
    rpc_call(Method::ServiceDelete, Some(json!({ "service_id": sid })))
        .await
        .expect("删除服务应成功");
    let list = rpc_call(Method::ServiceList, None).await.unwrap();
    assert_eq!(
        list["services"].as_array().unwrap().len(),
        0,
        "删除后列表应为空"
    );

    // ===== 8. config.get 往返 =====
    let cfg = rpc_call(Method::ConfigGet, None).await.unwrap();
    // v2.2.0 起 schema 版本为 2（新增 mem_alert_mb / HealthKind::Command）
    assert_eq!(cfg["services"]["version"], json!(2));

    // ===== 9. config.export / config.import 往返 =====
    let exported = rpc_call(Method::ConfigExport, None).await.unwrap();
    assert!(exported["services"].is_object(), "导出应含 services 配置");
    assert!(exported["app"].is_object(), "导出应含 app 配置");
    assert!(exported["exported_at"].as_str().is_some());
    let imported = rpc_call(
        Method::ConfigImport,
        Some(json!({
            "services": exported["services"],
            "app": exported["app"],
        })),
    )
    .await
    .expect("导入导出的配置应成功");
    assert_eq!(imported["ok"], json!(true));
    // 无效导入应被拒绝
    let bad = rpc_call(Method::ConfigImport, Some(json!({}))).await;
    assert!(bad.is_err(), "缺少 services 字段的导入应报错");

    // ===== 10. 事件流推送（event.subscribe 长连接）=====
    // 订阅连接：发送 event.subscribe 后保持连接读事件帧
    let mut sub_pipe = ClientOptions::new().open(test_pipe()).unwrap();
    let sub_req = Request::new(9001u64, Method::EventSubscribe, None);
    codec::write_frame(&mut sub_pipe, &sub_req).await.unwrap();
    let sub_resp: Response = codec::read_frame(&mut sub_pipe).await.unwrap();
    assert_eq!(sub_resp.into_result().unwrap()["mode"], json!("stream"));

    // 另一连接触发 config.changed 事件
    rpc_call(Method::ServiceCreate, Some(json!({ "service": svc2() })))
        .await
        .expect("创建第二个服务应成功");

    // 订阅连接应在超时前收到事件帧
    let read_ev = async { codec::read_frame::<serde_json::Value, _>(&mut sub_pipe).await };
    let ev = tokio::time::timeout(Duration::from_secs(5), read_ev)
        .await
        .expect("5 秒内应收到事件")
        .expect("事件帧应有效");
    assert_eq!(ev["topic"], json!("config.changed"));
    assert!(ev["ts"].as_str().is_some());

    // ===== 11. 守护进程日志（daemon.logs / daemon.logs.clear）=====
    let logs = rpc_call(Method::DaemonLogs, Some(json!({"tail": 10})))
        .await
        .expect("daemon.logs 应成功");
    assert!(logs["lines"].is_array());
    assert!(logs["total"].is_u64());
    let cleared = rpc_call(Method::DaemonLogsClear, None)
        .await
        .expect("daemon.logs.clear 应成功");
    assert_eq!(cleared["ok"], json!(true));
    // 清空后再读应为 0 行
    let after = rpc_call(Method::DaemonLogs, None)
        .await
        .expect("清空后再读应成功");
    assert_eq!(after["total"], json!(0));

    // ===== 清理 =====
    let _ = std::fs::remove_dir_all(&state.dirs.root);
}

/// 第二个测试服务的精简定义
fn svc2() -> ServiceDefinition {
    ServiceDefinition::new(
        "事件测试服务",
        std::path::PathBuf::from("C:\\Windows\\System32\\PING.EXE"),
    )
}

/// 检查进程是否存在
fn process_alive(pid: u32) -> bool {
    use std::process::{Command, Stdio};
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .stdin(Stdio::null())
        .creation_flags(0x0800_0000)
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
        Err(_) => false,
    }
}
