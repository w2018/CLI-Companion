//! 本机只读状态页（v2.2.0）
//!
//! - 仅绑定 127.0.0.1：不出本机，浏览器可看，局域网不可达
//! - 只读：仅 GET，仅展示服务名/状态/资源占用，不含环境变量（连名称也不含）、
//!   不含任何启停操作能力
//! - tokio 手写极简 HTTP（零新依赖），不解析/不回显请求内容，天然免疫注入

use crate::state::AppState;
use cli_companion_domain::{RuntimeState, ServicesConfig};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 按配置拉起状态页服务（失败只记日志，不影响 daemon）
pub fn spawn_if_enabled(state: AppState, enabled: bool, port: u16) {
    if !enabled {
        return;
    }
    tokio::spawn(async move {
        if let Err(e) = run_server(state, port).await {
            tracing::warn!("状态页退出: {e}");
        }
    });
}

/// 主循环：接受连接并逐个处理
pub async fn run_server(state: AppState, port: u16) -> std::io::Result<()> {
    // 显式只绑回环地址：状态页永远不出本机
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    tracing::info!(port, "状态页已启动（仅本机 127.0.0.1 可访问）");
    run_server_on(state, listener).await
}

/// 在给定监听器上运行（测试传端口 0 的系统分配监听器）
pub async fn run_server_on(state: AppState, listener: TcpListener) -> std::io::Result<()> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let st = state.clone();
        tokio::spawn(async move {
            let _ = handle_conn(&mut stream, &st).await;
        });
    }
}

/// 单连接：读请求头（忽略 body）→ 路由 → 响应后立即关闭
async fn handle_conn(stream: &mut TcpStream, state: &AppState) -> std::io::Result<()> {
    let mut buf = [0u8; 2048];
    let n = stream.read(&mut buf).await?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let first_line = req.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    let (status, content_type, body) = match (method, path) {
        ("GET", "/api/status") => (
            "200 OK",
            "application/json; charset=utf-8",
            build_status_json(
                &state.services().await,
                &state.manager.all_runtimes(),
                env!("CARGO_PKG_VERSION"),
                state.as_service,
            )
            .to_string(),
        ),
        ("GET", "/") | ("GET", "/index.html") => (
            "200 OK".into(),
            "text/html; charset=utf-8".into(),
            PAGE_HTML.to_string(),
        ),
        ("GET", _) => (
            "404 Not Found".into(),
            "text/plain; charset=utf-8".into(),
            "not found".into(),
        ),
        _ => (
            "405 Method Not Allowed".into(),
            "text/plain; charset=utf-8".into(),
            "read-only".into(),
        ),
    };
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// 组装只读状态 JSON（纯函数，单测验证不泄漏机密）
///
/// 仅暴露：服务名/说明/状态/PID/启动时间/CPU/内存/告警阈值标记。
pub fn build_status_json(
    cfg: &ServicesConfig,
    runtimes: &std::collections::HashMap<cli_companion_domain::ServiceId, RuntimeState>,
    daemon_version: &str,
    as_service: bool,
) -> serde_json::Value {
    let services: Vec<serde_json::Value> = cfg
        .services
        .iter()
        .map(|svc| {
            let rt = runtimes.get(&svc.id);
            json!({
                "name": svc.name,
                "description": svc.description,
                "autostart": svc.autostart,
                "status": rt.map(|r| r.status).unwrap_or(cli_companion_domain::ServiceStatus::Stopped),
                "pid": rt.and_then(|r| r.pid),
                "started_at": rt.and_then(|r| r.started_at),
                "cpu_percent": rt.and_then(|r| r.cpu_percent),
                "mem_bytes": rt.and_then(|r| r.mem_bytes),
            })
        })
        .collect();
    json!({
        "daemon_version": daemon_version,
        "running_as_service": as_service,
        "ts": chrono::Utc::now().to_rfc3339(),
        "services": services,
    })
}

/// 极简单页：fetch /api/status 每 3 秒刷新
const PAGE_HTML: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>CLI Companion 状态</title>
<style>
body{font-family:system-ui,sans-serif;background:#f6f7f9;color:#1c1e21;margin:0;padding:24px}
h1{font-size:20px}table{border-collapse:collapse;width:100%;background:#fff;border-radius:12px;overflow:hidden}
th,td{padding:10px 14px;text-align:left;border-bottom:1px solid #eceef1;font-size:14px}
th{background:#fafbfc;color:#5a6070}.ok{color:#15803d}.err{color:#b91c1c}.muted{color:#8a8f98}
</style></head><body>
<h1>CLI Companion 服务状态 <span class="muted" style="font-size:12px">只读 · 每 3 秒刷新</span></h1>
<table id="t"><thead><tr><th>服务</th><th>状态</th><th>PID</th><th>CPU</th><th>内存</th></tr></thead><tbody></tbody></table>
<script>
const BADGE={"running":"运行中","stopped":"已停止","failed":"异常","starting":"启动中","stopping":"停止中","restarting":"重启中"};
async function refresh(){
 try{
  const r=await fetch('/api/status');const d=await r.json();
  const tb=document.querySelector('#t tbody');tb.innerHTML='';
  for(const s of d.services){
    const tr=document.createElement('tr');
    const cls=s.status==='running'?'ok':(s.status==='failed'?'err':'muted');
    tr.innerHTML=`<td>${s.name}</td><td class="${cls}">${BADGE[s.status]||s.status}</td>
      <td class="muted">${s.pid??'—'}</td>
      <td class="muted">${s.cpu_percent!=null?s.cpu_percent.toFixed(1)+'%':'—'}</td>
      <td class="muted">${s.mem_bytes!=null?(s.mem_bytes/1048576).toFixed(0)+' MB':'—'}</td>`;
    tb.appendChild(tr);
  }
 }catch(e){document.title='状态获取失败';}
}
refresh();setInterval(refresh,3000);
</script></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use cli_companion_domain::{EnvVar, ServiceDefinition, ServiceId, ServiceStatus};
    use std::collections::HashMap;

    #[test]
    fn 状态json不含环境变量与机密() {
        let mut svc = ServiceDefinition::new("本地代理", "a.exe".into());
        svc.env = vec![EnvVar {
            name: "TOKEN".into(),
            value: "super-secret".into(),
            secret: true,
        }];
        let mut cfg = ServicesConfig::default();
        let svc_id = svc.id;
        cfg.services.push(svc);
        let mut runtimes = HashMap::new();
        let mut rt = RuntimeState::default();
        rt.status = ServiceStatus::Running;
        rt.pid = Some(42);
        runtimes.insert(svc_id, rt);

        let v = build_status_json(&cfg, &runtimes, "2.2.0", false);
        let s = v.to_string();
        // 环境变量名与值都不出现在状态里
        assert!(!s.contains("TOKEN"));
        assert!(!s.contains("super-secret"));
        assert!(!s.contains("exe"));
        assert_eq!(v["services"][0]["name"], "本地代理");
        assert_eq!(v["services"][0]["pid"], 42);
        assert_eq!(v["services"][0]["status"], "running");
    }

    /// 真实 HTTP 集成验证（随机端口 GET /api/status）
    #[tokio::test]
    async fn 随机端口可访问且返回json() {
        let dirs = crate::dirs::DataDirs::resolve(Some(std::env::temp_dir().join(format!(
            "cc-http-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))));
        let state = AppState {
            as_service: false,
            manager: std::sync::Arc::new(crate::manager::ServiceManager::new(
                dirs.clone(),
                std::sync::Arc::new(crate::events::new_bus()),
                false,
            )),
            config: std::sync::Arc::new(tokio::sync::Mutex::new(crate::state::ConfigStore {
                services: ServicesConfig::default(),
                app: crate::app_config::AppConfig::default(),
                secrets: Default::default(),
            })),
            sync: std::sync::Arc::new(crate::sync::SyncEngine::new()),
            events: std::sync::Arc::new(crate::events::new_bus()),
            shutdown: std::sync::Arc::new(tokio::sync::Notify::new()),
            dirs,
        };
        // 端口 0 由系统分配
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = run_server_on(state, listener).await;
        });
        // 发起真实 HTTP 请求
        use tokio::net::TcpStream;
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(b"GET /api/status HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        let _ =
            tokio::time::timeout(std::time::Duration::from_secs(3), s.read_to_end(&mut buf)).await;
        let text = String::from_utf8_lossy(&buf);
        assert!(text.starts_with("HTTP/1.1 200 OK"));
        assert!(text.contains("\"services\""));
        assert!(text.contains("\"daemon_version\""));
    }
}
