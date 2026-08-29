//! 本机只读状态页（v2.2.0）
//!
//! - 仅绑定 127.0.0.1：不出本机，浏览器可看，局域网不可达
//! - 只读：仅 GET，仅展示服务概要信息，不含环境变量（连名称也不含）、
//!   不含任何启停操作能力
//! - tokio 手写极简 HTTP（零新依赖），不解析/不回显请求内容，天然免疫注入
//! - 动态生效：监督任务随 config.changed 事件启停/换端口，无需重启 daemon

use crate::state::AppState;
use cli_companion_domain::{RuntimeState, ServiceStatus, ServicesConfig};
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// 监督任务：按最新 app 配置拉起/停止状态页（随 config.changed 即时生效）
pub fn spawn_supervisor(state: AppState) {
    tokio::spawn(async move {
        let mut rx = state.events.subscribe();
        // 当前运行中的实例：(端口, 停止信号)
        let mut running: Option<(u16, Arc<Notify>)> = None;
        loop {
            // 期望状态
            let app = state.app().await;
            let want = if app.status_page.enabled {
                Some(app.status_page.port)
            } else {
                None
            };
            // 收敛到期望状态
            match (want, running.take()) {
                (Some(port), Some((cur_port, shutdown))) => {
                    if cur_port == port {
                        running = Some((cur_port, shutdown)); // 无变化，继续
                    } else {
                        shutdown.notify_waiters(); // 换端口：停旧起新
                        running = start(state.clone(), port);
                    }
                }
                (Some(port), None) => running = start(state.clone(), port),
                (None, Some((_, shutdown))) => shutdown.notify_waiters(),
                (None, None) => {}
            }
            // 等下一次配置变更或 daemon 关闭
            tokio::select! {
                ev = rx.recv() => match ev {
                    Ok(e) if e.topic == cli_companion_protocol::EventTopic::ConfigChanged => continue,
                    Ok(_) => continue, // 其他事件不触发
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break, // 总线关闭（daemon 退出）
                },
                _ = state.shutdown.notified() => {
                    if let Some((_, shutdown)) = running.take() {
                        shutdown.notify_waiters();
                    }
                    break;
                }
            }
        }
    });
}

/// 起一个状态页实例；端口被占用等失败只记日志（保持未运行，等待下次变更重试）
fn start(state: AppState, port: u16) -> Option<(u16, Arc<Notify>)> {
    let shutdown = Arc::new(Notify::new());
    let sd = shutdown.clone();
    tokio::spawn(async move {
        if let Err(e) = run_server(state, port, sd).await {
            tracing::warn!("状态页退出: {e}");
        }
    });
    Some((port, shutdown))
}

/// 主循环：接受连接并逐个处理；收到停止信号即退出
pub async fn run_server(state: AppState, port: u16, shutdown: Arc<Notify>) -> std::io::Result<()> {
    // 显式只绑回环地址：状态页永远不出本机
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    tracing::info!(port, "状态页已启动（仅本机 127.0.0.1 可访问）");
    run_server_on(state, listener, shutdown).await
}

/// 在给定监听器上运行（测试传端口 0 的系统分配监听器）
pub async fn run_server_on(
    state: AppState,
    listener: TcpListener,
    shutdown: Arc<Notify>,
) -> std::io::Result<()> {
    loop {
        tokio::select! {
            r = listener.accept() => {
                let (mut stream, _) = match r {
                    Ok(x) => x,
                    Err(e) => {
                        tracing::warn!("状态页接受连接失败: {e}");
                        continue;
                    }
                };
                let st = state.clone();
                tokio::spawn(async move {
                    let _ = handle_conn(&mut stream, &st).await;
                });
            }
            _ = shutdown.notified() => {
                tracing::info!("状态页已停止");
                return Ok(());
            }
        }
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
        ("GET", "/api/status") => {
            let cfg = state.services().await;
            let runtimes = state.manager.all_runtimes();
            let app = state.app().await;
            (
                "200 OK",
                "application/json; charset=utf-8",
                build_status_json(
                    &cfg,
                    &runtimes,
                    env!("CARGO_PKG_VERSION"),
                    state.as_service,
                    &app.status_page,
                )
                .to_string(),
            )
        }
        ("GET", "/") | ("GET", "/index.html") => {
            ("200 OK", "text/html; charset=utf-8", PAGE_HTML.to_string())
        }
        ("GET", _) => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found".into(),
        ),
        _ => (
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
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
/// 仅暴露概要信息：服务名/说明/状态/进程信息/资源占用/重启统计；
/// 不含 exe、参数、环境变量（名称也不含）、WebDAV 配置等。
pub fn build_status_json(
    cfg: &ServicesConfig,
    runtimes: &std::collections::HashMap<cli_companion_domain::ServiceId, RuntimeState>,
    daemon_version: &str,
    as_service: bool,
    status_page: &crate::app_config::StatusPageSettings,
) -> serde_json::Value {
    let mut running = 0u32;
    let mut failed = 0u32;
    let services: Vec<serde_json::Value> = cfg
        .services
        .iter()
        .map(|svc| {
            let rt = runtimes.get(&svc.id);
            let status = rt.map(|r| r.status).unwrap_or(ServiceStatus::Stopped);
            match status {
                ServiceStatus::Running => running += 1,
                ServiceStatus::Failed => failed += 1,
                _ => {}
            }
            json!({
                "name": svc.name,
                "description": svc.description,
                "autostart": svc.autostart,
                "enabled": svc.enabled,
                "status": status,
                "pid": rt.and_then(|r| r.pid),
                "started_at": rt.and_then(|r| r.started_at),
                "cpu_percent": rt.and_then(|r| r.cpu_percent),
                "mem_bytes": rt.and_then(|r| r.mem_bytes),
                "restart_count": rt.map(|r| r.restart_count).unwrap_or(0),
                "last_exit_code": rt.and_then(|r| r.last_exit_code),
                "last_health": rt.and_then(|r| r.last_health.clone()),
            })
        })
        .collect();
    json!({
        "daemon_version": daemon_version,
        "running_as_service": as_service,
        "status_page_port": status_page.port,
        "ts": chrono::Utc::now().to_rfc3339(),
        "summary": {
            "total": services.len(),
            "running": running,
            "failed": failed,
            "stopped": services.len() as u32 - running - failed,
        },
        "services": services,
    })
}

/// 信息页：汇总卡片 + 明细表，3 秒自动刷新
const PAGE_HTML: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>CLI Companion 服务状态</title>
<style>
:root{color-scheme:light}
*{box-sizing:border-box}
body{font-family:"Segoe UI",system-ui,sans-serif;background:linear-gradient(180deg,#eef1f6,#e8ecf2);color:#1c1e21;margin:0;padding:28px 20px}
.wrap{max-width:980px;margin:0 auto}
h1{font-size:22px;margin:0 0 2px}
.sub{color:#6b7280;font-size:12px;margin-bottom:18px}
.cards{display:grid;grid-template-columns:repeat(4,1fr);gap:12px;margin-bottom:16px}
.card{background:#fff;border:1px solid #e3e7ee;border-radius:14px;padding:14px 16px}
.card .num{font-size:26px;font-weight:700}
.card .lbl{font-size:12px;color:#6b7280}
.card.ok .num{color:#15803d}.card.err .num{color:#b91c1c}.card.total .num{color:#1d4ed8}
.panel{background:#fff;border:1px solid #e3e7ee;border-radius:14px;overflow:hidden}
table{border-collapse:collapse;width:100%}
th,td{padding:10px 14px;text-align:left;border-bottom:1px solid #eef0f4;font-size:13px;white-space:nowrap}
td.desc{max-width:260px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:#6b7280}
th{background:#f8fafc;color:#5a6070;font-weight:600}
tr:last-child td{border-bottom:none}
.badge{display:inline-block;border-radius:999px;padding:2px 10px;font-size:12px;border:1px solid}
.b-run{background:#dcfce7;color:#15803d;border-color:#bbf7d0}
.b-fail{background:#fee2e2;color:#b91c1c;border-color:#fecaca}
.b-stop{background:#f1f5f9;color:#64748b;border-color:#e2e8f0}
.b-other{background:#fef9c3;color:#a16207;border-color:#fde68a}
.mono{font-family:Consolas,monospace}
.muted{color:#9aa1ad}
.foot{margin-top:14px;font-size:11px;color:#9aa1ad;text-align:center}
@media(max-width:720px){.cards{grid-template-columns:repeat(2,1fr)}}
</style></head><body><div class="wrap">
<h1>🖥️ CLI Companion 服务状态</h1>
<p class="sub">本机只读视图 · 自动每 3 秒刷新 · <span id="meta">—</span></p>
<div class="cards">
  <div class="card total"><div class="num" id="c-total">—</div><div class="lbl">服务总数</div></div>
  <div class="card ok"><div class="num" id="c-run">—</div><div class="lbl">运行中</div></div>
  <div class="card err"><div class="num" id="c-fail">—</div><div class="lbl">异常</div></div>
  <div class="card"><div class="num" id="c-stop">—</div><div class="lbl">已停止</div></div>
</div>
<div class="panel">
<table><thead><tr>
<th>服务</th><th>状态</th><th>PID</th><th>运行时长</th><th>CPU</th><th>内存</th><th>重启次数</th><th>最近退出码</th><th>说明</th>
</tr></thead><tbody id="tb"></tbody></table>
</div>
<p class="foot">只读页面：不提供任何操作 · 数据来自本机 daemon · <span id="gen">—</span></p>
<script>
const BADGE={running:["运行中","b-run"],failed:["异常","b-fail"],stopped:["已停止","b-stop"]};
function dur(iso){if(!iso)return"—";const s=Math.max(0,(Date.now()-new Date(iso).getTime())/1000);
if(s<60)return Math.floor(s)+"秒";const m=Math.floor(s/60);if(m<60)return m+"分"+Math.floor(s%60)+"秒";
const h=Math.floor(m/60);if(h<24)return h+"小时"+(m%60)+"分";return Math.floor(h/24)+"天"+(h%24)+"小时"}
function esc(s){return String(s??"").replace(/[&<>"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]))}
async function refresh(){
 try{
  const d=await (await fetch('/api/status')).json();
  document.getElementById('meta').textContent='daemon v'+d.daemon_version+(d.running_as_service?'（Windows 服务）':'（后台进程）');
  document.getElementById('c-total').textContent=d.summary.total;
  document.getElementById('c-run').textContent=d.summary.running;
  document.getElementById('c-fail').textContent=d.summary.failed;
  document.getElementById('c-stop').textContent=d.summary.stopped;
  const tb=document.getElementById('tb');tb.innerHTML='';
  if(!d.services.length){tb.innerHTML='<tr><td colspan="9" class="muted" style="text-align:center;padding:24px">暂无服务</td></tr>';}
  for(const s of d.services){
    const[label,cls]=BADGE[s.status]||[s.status,'b-other'];
    const tr=document.createElement('tr');
    tr.innerHTML=`<td><b>${esc(s.name)}</b>${s.autostart?' <span class="badge b-other" style="font-size:10px;padding:1px 6px">自启</span>':''}</td>
      <td><span class="badge ${cls}">${label}</span></td>
      <td class="mono muted">${s.pid??'—'}</td>
      <td class="muted">${s.status==='running'?dur(s.started_at):'—'}</td>
      <td class="mono muted">${s.cpu_percent!=null?s.cpu_percent.toFixed(1)+'%':'—'}</td>
      <td class="mono muted">${s.mem_bytes!=null?(s.mem_bytes/1048576).toFixed(0)+' MB':'—'}</td>
      <td class="mono muted">${s.restart_count}</td>
      <td class="mono ${s.last_exit_code!=null&&s.last_exit_code!==0?'':'muted'}">${s.last_exit_code??'—'}</td>
      <td class="desc">${esc(s.description||'—')}</td>`;
    tb.appendChild(tr);
  }
  document.getElementById('gen').textContent='生成于 '+new Date(d.ts).toLocaleTimeString('zh-CN',{hour12:false});
 }catch(e){document.getElementById('meta').textContent='状态获取失败：'+e}
}
refresh();setInterval(refresh,3000);
</script></div></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use cli_companion_domain::{EnvVar, ServiceDefinition, ServiceStatus};
    use std::collections::HashMap;

    #[test]
    fn 状态json丰富且不含敏感信息() {
        let mut svc = ServiceDefinition::new("本地代理", "a.exe".into());
        svc.description = "演示服务".into();
        svc.env = vec![EnvVar {
            name: "TOKEN".into(),
            value: "super-secret".into(),
            secret: true,
        }];
        let mut cfg = ServicesConfig::default();
        let svc_id = svc.id;
        cfg.services.push(svc);
        let mut runtimes = HashMap::new();
        let rt = RuntimeState {
            status: ServiceStatus::Running,
            pid: Some(42),
            restart_count: 3,
            last_exit_code: Some(7),
            ..Default::default()
        };
        runtimes.insert(svc_id, rt);

        let v = build_status_json(
            &cfg,
            &runtimes,
            "2.2.0",
            false,
            &crate::app_config::StatusPageSettings::default(),
        );
        let s = v.to_string();
        // 敏感信息不出现
        assert!(!s.contains("TOKEN"));
        assert!(!s.contains("super-secret"));
        assert!(!s.contains("exe"));
        // 丰富信息存在
        assert_eq!(v["services"][0]["name"], "本地代理");
        assert_eq!(v["services"][0]["pid"], 42);
        assert_eq!(v["services"][0]["restart_count"], 3);
        assert_eq!(v["services"][0]["last_exit_code"], 7);
        assert_eq!(v["summary"]["total"], 1);
        assert_eq!(v["summary"]["running"], 1);
        assert!(v["status_page_port"].is_number());
    }

    /// 真实 HTTP 集成验证（随机端口 GET /api/status 与 /）
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
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = run_server_on(state, listener, Arc::new(Notify::new())).await;
        });
        // /api/status
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
        assert!(text.contains("\"summary\""));
        assert!(text.contains("\"daemon_version\""));
        // / HTML 页
        let mut s2 = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s2.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf2 = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), s2.read_to_end(&mut buf2))
            .await;
        let html = String::from_utf8_lossy(&buf2);
        assert!(html.starts_with("HTTP/1.1 200 OK"));
        assert!(html.contains("服务状态"));
    }
}
