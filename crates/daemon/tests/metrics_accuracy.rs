//! 集成测试：GPU / 磁盘 / 网络 / 进程树 CPU 指标端到端准确性
//!
//! 真实链路：daemon 管道 RPC + 真实子进程工作负载，验证 service.metrics
//! 上报的速率与占用反映实际 I/O，防止口径错误或恒 0 的回归：
//! 1. 网络+磁盘：子进程 curl 回环下载 8MB（分块慢速下发，跨采样窗口保持连接），
//!    断言观测到网络接收速率与磁盘写速率，且累计接收量与载荷量级吻合；
//! 2. 进程树 CPU：根进程 cmd 空闲等待、powershell 子进程满载——
//!    回归测试"CPU 恒 0.0%"（旧逻辑只采根进程的缺陷）。
//!
//! 两个场景串联在单个 #[tokio::test] 内：管道名每进程唯一，
//! 并行用例会争用同一管道服务端（与 pipe_rpc 同因）。

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cli_companion_daemon::state::AppState;
use cli_companion_domain::{Arg, ArgKind, ConsoleMode, ServiceDefinition};
use cli_companion_protocol::codec;
use cli_companion_protocol::method::Method;
use cli_companion_protocol::{Request, Response};
use serde_json::json;
use tokio::net::windows::named_pipe::ClientOptions;
use tokio::sync::Mutex as AsyncMutex;

// ===== 测试辅助（每测试进程唯一管道名） =====

fn test_pipe() -> &'static str {
    static PIPE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PIPE.get_or_init(|| format!(r"\\.\pipe\cli-companion-mtest-{}", std::process::id()))
}

fn test_state(tag: &str) -> AppState {
    let dirs = cli_companion_daemon::dirs::DataDirs::resolve(Some(
        std::env::temp_dir().join(format!("cli-comp-ma-{}-{}", tag, std::process::id())),
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

async fn rpc_call(
    method: Method,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, cli_companion_protocol::RpcError> {
    use cli_companion_protocol::error::ErrorCode;
    let mut pipe = ClientOptions::new().open(test_pipe()).map_err(|_| {
        cli_companion_protocol::RpcError::new(ErrorCode::DaemonUnavailable, "daemon 不可达")
    })?;
    static ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(20000);
    let req = Request::new(
        ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        method,
        params,
    );
    codec::write_frame(&mut pipe, &req).await?;
    let resp: Response = codec::read_frame(&mut pipe).await?;
    resp.into_result()
}

async fn wait_pipe_ready() {
    for _ in 0..50 {
        if rpc_call(Method::SystemPing, None).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("ping 100ms×50 后仍不可用");
}

/// 位置参数构造
fn pos(value: &str, idx: usize) -> Arg {
    Arg {
        id: format!("ma{idx}"),
        key: String::new(),
        value: Some(value.to_string()),
        enabled: true,
        kind: ArgKind::Positional,
        description: String::new(),
    }
}

async fn create_and_start(svc: ServiceDefinition) -> String {
    let created = rpc_call(Method::ServiceCreate, Some(json!({ "service": svc })))
        .await
        .expect("创建服务应成功");
    let sid = created["service"]["id"].as_str().unwrap().to_string();
    rpc_call(Method::ServiceStart, Some(json!({ "service_id": sid })))
        .await
        .expect("启动服务应成功");
    // 等待进入 running
    for _ in 0..50 {
        let list = rpc_call(Method::ServiceList, None).await.unwrap();
        let running = list["services"].as_array().unwrap().iter().any(|s| {
            s["service"]["id"] == json!(sid) && s["runtime"]["status"] == json!("running")
        });
        if running {
            return sid;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("服务应进入 running 状态");
}

async fn stop_and_delete(sid: &str) {
    let _ = rpc_call(Method::ServiceStop, Some(json!({ "service_id": sid }))).await;
    let _ = rpc_call(Method::ServiceDelete, Some(json!({ "service_id": sid }))).await;
}

/// 从 service.metrics 里取指定服务的指标条目
async fn metric_entry(sid: &str) -> Option<serde_json::Value> {
    let m = rpc_call(Method::ServiceMetrics, None).await.ok()?;
    m["metrics"]
        .as_array()?
        .iter()
        .find(|e| e["service_id"] == json!(sid))
        .cloned()
}

// ===== 场景 1：网络 + 磁盘指标端到端准确 =====

async fn 场景_网络与磁盘(state: &AppState) {
    // 慢速分块下发 8MB（每块 512KB，块间 400ms，总时长 ~6.5s），
    // 保证连接横跨 ≥3 个 2s 采样窗口且 curl 持续落盘
    const TOTAL: usize = 8 * 1024 * 1024;
    const CHUNK: usize = 512 * 1024;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定应成功");
    let port = listener.local_addr().unwrap().port();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let sflag = stop_flag.clone();
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).ok();
        let mut sock = loop {
            if sflag.load(Ordering::Relaxed) {
                return;
            }
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => return,
            }
        };
        sock.set_nonblocking(false).ok();
        sock.set_read_timeout(Some(Duration::from_secs(10))).ok();
        // 读完 HTTP 请求头（curl 的 GET /\r\n\r\n），再回标准 200 响应
        let mut req = Vec::new();
        let mut byte = [0u8; 1];
        while let Ok(1) = sock.read(&mut byte) {
            req.push(byte[0]);
            if req.ends_with(b"\r\n\r\n") || req.len() > 64 * 1024 {
                break;
            }
        }
        let header =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {TOTAL}\r\nConnection: close\r\n\r\n");
        if sock.write_all(header.as_bytes()).is_err() {
            return;
        }
        let buf = vec![b'x'; 64 * 1024];
        let mut sent = 0usize;
        while sent < TOTAL {
            let mut n = CHUNK.min(TOTAL - sent);
            while n > 0 {
                let w = n.min(buf.len());
                if sock.write_all(&buf[..w]).is_err() {
                    return;
                }
                n -= w;
                sent += w;
            }
            if sent < TOTAL {
                std::thread::sleep(Duration::from_millis(400));
            }
        }
        let _ = sock.shutdown(std::net::Shutdown::Write);
        let mut tmp = [0u8; 64];
        let _ = sock.read(&mut tmp); // 等待对端关闭
    });

    let out_path = std::env::temp_dir()
        .join(format!("cli-comp-ma-dl-{}.bin", std::process::id()))
        .to_string_lossy()
        .to_string();
    let mut svc = ServiceDefinition::new(
        "指标准确-网络磁盘",
        std::path::PathBuf::from("C:\\Windows\\System32\\curl.exe"),
    );
    let parts = [
        "-s",
        "-o",
        out_path.as_str(),
        &format!("http://127.0.0.1:{port}/"),
    ];
    svc.args = parts.iter().enumerate().map(|(i, p)| pos(p, i)).collect();
    svc.console.mode = ConsoleMode::NoConsole;

    let sid = create_and_start(svc).await;

    // 每 2.2s 轮询一次（≥ 采样周期 2s，每个速率窗口只计一次）
    let mut max_rx_rate = 0u64;
    let mut max_disk_write = 0u64;
    let mut rx_accum = 0u64; // Σ 速率 × 2s 窗口 ≈ 累计接收字节
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(2200)).await;
        if let Some(entry) = metric_entry(&sid).await {
            if let Some(v) = entry["net_rx_bytes_per_sec"].as_u64() {
                max_rx_rate = max_rx_rate.max(v);
                rx_accum += v * 2;
            }
            if let Some(v) = entry["disk_write_bytes_per_sec"].as_u64() {
                max_disk_write = max_disk_write.max(v);
            }
            // GPU / 内存百分比若上报必须落在界内
            if let Some(g) = entry["gpu_percent"].as_f64() {
                assert!((0.0..=100.0).contains(&g), "GPU 利用率越界: {g}");
            }
            if let Some(m) = entry["mem_percent"].as_f64() {
                assert!((0.0..=100.0).contains(&m), "内存百分比越界: {m}");
            }
            // 累计达标（≥ 载荷 1/4）且观察到磁盘写 → 提前结束轮询
            if rx_accum >= (TOTAL / 4) as u64 && max_disk_write > 0 {
                break;
            }
        }
    }

    // 先清理工作负载再断言（避免失败时传输进程泄漏）
    stop_and_delete(&sid).await;
    stop_flag.store(true, Ordering::Relaxed);
    let _ = std::fs::remove_file(&out_path);
    let _ = server.join();

    assert!(
        max_rx_rate > 256 * 1024,
        "应观测到明显的网络接收速率（下载 8MB）: max={max_rx_rate} B/s"
    );
    assert!(
        max_disk_write > 0,
        "应观测到磁盘写速率（curl 落盘）: max={max_disk_write}"
    );
    assert!(
        rx_accum >= (TOTAL / 4) as u64,
        "累计网络接收应达到载荷四分之一以上（连接启用统计前的窗口按口径漏计，\
         平台层已验证启用后差分精确无损）: accum={rx_accum} B vs 载荷 {TOTAL} B"
    );
    assert!(
        rx_accum <= (TOTAL * 2) as u64,
        "累计网络接收不应显著超过载荷（防重复计数）: accum={rx_accum} B vs 载荷 {TOTAL} B"
    );

    let _ = std::fs::remove_dir_all(&state.dirs.root);
}

// ===== 场景 2：进程树 CPU 回归（根进程空闲、子进程满载） =====

async fn 场景_进程树cpu(state: &AppState) {
    // 根进程 cmd 启动 powershell 子进程满载后自身只做 ping 等待（近 0 CPU）：
    // 旧逻辑只采根进程 → 恒 0.0%；树聚合逻辑应能反映子进程占用
    let mut svc = ServiceDefinition::new(
        "指标准确-进程树CPU",
        std::path::PathBuf::from("C:\\Windows\\System32\\cmd.exe"),
    );
    let parts = [
        "/c",
        "start",
        "/b",
        "powershell",
        "-NoProfile",
        "-Command",
        "while($true){}",
        "&",
        "ping",
        "-n",
        "30",
        "127.0.0.1",
        ">nul",
    ];
    svc.args = parts.iter().enumerate().map(|(i, p)| pos(p, i)).collect();
    svc.console.mode = ConsoleMode::NoConsole;

    let sid = create_and_start(svc).await;

    // 轮询最长 25s，等待 powershell 启动并跨过 2 个采样窗口
    let mut max_cpu = 0.0f64;
    let deadline = Instant::now() + Duration::from_secs(25);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(600)).await;
        if let Some(entry) = metric_entry(&sid).await {
            if let Some(cpu) = entry["cpu_percent"].as_f64() {
                max_cpu = max_cpu.max(cpu);
                if max_cpu > 0.5 {
                    break;
                }
            }
        }
    }

    // 先停止满载负载再断言
    stop_and_delete(&sid).await;

    assert!(
        max_cpu > 0.5,
        "进程树 CPU 应反映 powershell 子进程的真实占用（根 cmd 空闲），\
         若恒为 0 说明树聚合回归: max={max_cpu}%"
    );

    let _ = std::fs::remove_dir_all(&state.dirs.root);
}

// ===== 单测试串联两个场景 =====

#[tokio::test(flavor = "multi_thread")]
async fn 网络磁盘与进程树cpu指标端到端准确() {
    let state = test_state("acc");
    {
        let st = state.clone();
        tokio::spawn(async move {
            let _ = cli_companion_daemon::rpc::run_pipe_server_on(st, test_pipe()).await;
        });
    }
    wait_pipe_ready().await;

    场景_网络与磁盘(&state).await;
    场景_进程树cpu(&state).await;
}
