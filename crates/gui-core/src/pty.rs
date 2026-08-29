//! 内嵌终端（v2.2.0 任务9）：ConPTY 封装 + Tauri 命令
//!
//! - 核心 [`spawn_pty`] 与 Tauri 解耦，可独立集成测试（真实 ConPTY 回显验证）
//! - 环境复用 terminal.rs 的服务环境合并逻辑（系统环境 + 服务覆盖，不改系统环境变量）
//! - 输出经 `pty-output:<id>` 事件推送前端 xterm.js；EOF 即会话结束 `pty-exit:<id>`

use crate::terminal::{build_env_overrides, expand_percent, fetch_service_env, shell_command};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::Emitter;

/// 全部活跃 PTY 会话
static SESSIONS: Mutex<Option<HashMap<u64, PtySession>>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// 每会话输出回放缓冲上限（重进页面时回放，256KB ≈ 数千行）
const BACKLOG_CAP: usize = 256 * 1024;

/// 一个 PTY 会话持有的资源
struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// 所属服务 ID（pty_attach 查找用）
    service_id: String,
    /// 输出回放缓冲（离开页面后继续积累，重进时整体回放恢复屏幕）
    backlog: std::sync::Arc<Mutex<Vec<u8>>>,
}

/// 追加到回放缓冲（超限从头部丢弃，近似环形）
fn backlog_append(buf: &Mutex<Vec<u8>>, chunk: &[u8]) {
    let mut b = buf.lock().unwrap();
    b.extend_from_slice(chunk);
    if b.len() > BACKLOG_CAP {
        let drop = b.len() - BACKLOG_CAP;
        // 丢到下一个完整 UTF-8 边界（避免回放时出现乱码）
        let mut cut = drop;
        while cut < b.len() && (b[cut] & 0xC0) == 0x80 {
            cut += 1;
        }
        b.drain(..cut);
    }
}

/// PTY 启动参数（与 Tauri 解耦，便于测试）
pub struct PtyConfig {
    pub program: String,
    pub args: Vec<String>,
    pub env_overrides: Vec<(String, String)>,
    pub cwd: Option<String>,
    pub rows: u16,
    pub cols: u16,
}

impl Default for PtyConfig {
    fn default() -> Self {
        Self {
            program: "cmd.exe".into(),
            args: vec!["/K".into()],
            env_overrides: Vec::new(),
            cwd: None,
            rows: 24,
            cols: 80,
        }
    }
}

/// 拉起 PTY 会话：返回会话 ID；输出经 `output_tx` 逐块送出（EOF 时通道关闭）
pub fn spawn_pty(
    service_id: &str,
    cfg: PtyConfig,
    output_tx: std::sync::mpsc::Sender<Vec<u8>>,
) -> Result<u64, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: cfg.rows,
            cols: cfg.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("创建伪终端失败: {e}"))?;
    let portable_pty::PtyPair { master, slave } = pair;

    let mut cmd = CommandBuilder::new(&cfg.program);
    cmd.args(&cfg.args);
    for (k, v) in &cfg.env_overrides {
        cmd.env(k, v);
    }
    if let Some(cwd) = &cfg.cwd {
        cmd.cwd(cwd);
    }
    let child = slave
        .spawn_command(cmd)
        .map_err(|e| format!("启动终端失败: {e}"))?;
    drop(slave); // slave 用完即弃：子进程持有其端

    let mut reader = master
        .try_clone_reader()
        .map_err(|e| format!("接管输出失败: {e}"))?;
    let writer = master
        .take_writer()
        .map_err(|e| format!("接管输入失败: {e}"))?;
    let killer = child.clone_killer();

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let backlog = std::sync::Arc::new(Mutex::new(Vec::new()));
    let backlog_pump = backlog.clone();
    // 输出泵：EOF（进程退出）时线程结束 → 通道关闭 → 调用方感知；
    // 每块先入回放缓冲（离开页面后仍持续积累，重进页面可恢复屏幕）
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    backlog_append(&backlog_pump, &buf[..n]);
                    if output_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    // child 本体仅用于关闭（wait 由 EOF 隐含）；主动保活句柄交由 killer 管理
    std::mem::forget(child);

    SESSIONS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(
            id,
            PtySession {
                master,
                writer,
                killer,
                service_id: service_id.to_string(),
                backlog,
            },
        );
    Ok(id)
}

/// 按服务查找既有会话：返回 (id, 回放缓冲)——前端重进页面时恢复屏幕
pub fn attach_by_service(service_id: &str) -> Option<(u64, String)> {
    let map = SESSIONS.lock().unwrap();
    let map = map.as_ref()?;
    for (id, s) in map {
        if s.service_id == service_id {
            let backlog = s.backlog.lock().unwrap();
            return Some((*id, String::from_utf8_lossy(&backlog).to_string()));
        }
    }
    None
}

/// 写入用户输入
fn pty_write(id: u64, data: &str) -> Result<(), String> {
    let mut map = SESSIONS.lock().unwrap();
    let map = map.as_mut().ok_or("PTY 表不可用")?;
    let s = map
        .get_mut(&id)
        .ok_or_else(|| format!("会话不存在: {id}"))?;
    s.writer
        .write_all(data.as_bytes())
        .map_err(|e| e.to_string())?;
    s.writer.flush().map_err(|e| e.to_string())
}

/// 调整尺寸
fn pty_resize(id: u64, rows: u16, cols: u16) -> Result<(), String> {
    let mut map = SESSIONS.lock().unwrap();
    let map = map.as_mut().ok_or("PTY 表不可用")?;
    let s = map
        .get_mut(&id)
        .ok_or_else(|| format!("会话不存在: {id}"))?;
    s.master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())
}

/// 关闭会话（终止子进程并回收资源）
fn pty_close(id: u64) -> Result<(), String> {
    let mut map = SESSIONS.lock().unwrap();
    if let Some(map) = map.as_mut() {
        if let Some(mut s) = map.remove(&id) {
            let _ = s.killer.kill();
        }
    }
    Ok(())
}

/// 打开内嵌终端：返回会话 ID；输出事件 `pty-output:<id>`，结束事件 `pty-exit:<id>`
#[tauri::command]
pub async fn pty_open(
    app: tauri::AppHandle,
    service_id: String,
    shell: Option<String>,
) -> Result<u64, String> {
    let ctx = fetch_service_env(&service_id).await?;
    let (program, args) = shell_command(shell.as_deref(), "CLI Companion");
    let cwd = ctx
        .working_dir
        .map(|wd| expand_percent(&wd))
        .filter(|wd| !wd.is_empty() && std::path::Path::new(wd).is_dir());
    let cfg = PtyConfig {
        program,
        args,
        env_overrides: build_env_overrides(&ctx.env_vars),
        cwd,
        rows: 24,
        cols: 80,
    };
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let id = spawn_pty(&service_id, cfg, tx)?;

    // 输出转发线程：逐块 emit 给前端；通道关闭 = 子进程退出 → 发结束事件并移除会话
    // （回放缓冲已在 spawn_pty 的输出泵内持续积累，与转发解耦）
    let app2 = app.clone();
    std::thread::spawn(move || {
        for chunk in rx {
            let text = String::from_utf8_lossy(&chunk).to_string();
            // emit 失败（GUI 整体退出）才停止转发；无监听者时事件自然丢弃，
            // 缓冲仍持续积累，保证重进页面可回放
            if app2.emit(&format!("pty-output:{id}"), text).is_err() {
                break;
            }
        }
        let _ = app2.emit(&format!("pty-exit:{id}"), ());
        if let Some(map) = SESSIONS.lock().unwrap().as_mut() {
            map.remove(&id);
        }
    });
    Ok(id)
}

/// 重进页面时恢复会话：按服务查找既有 PTY，返回 (id, 回放缓冲)；无则 None
#[tauri::command]
pub async fn pty_attach(service_id: String) -> Result<Option<(u64, String)>, String> {
    Ok(attach_by_service(&service_id))
}

#[tauri::command]
pub async fn pty_write_cmd(id: u64, data: String) -> Result<(), String> {
    pty_write(id, &data)
}

#[tauri::command]
pub async fn pty_resize_cmd(id: u64, rows: u16, cols: u16) -> Result<(), String> {
    pty_resize(id, rows, cols)
}

#[tauri::command]
pub async fn pty_close_cmd(id: u64) -> Result<(), String> {
    pty_close(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// 真实 ConPTY 交互验证：/K 保持会话 → writer 写入 echo → 读到回显（含输入路径与输出路径）
    #[test]
    fn conpty交互端到端() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let cfg = PtyConfig {
            program: "cmd.exe".into(),
            args: vec!["/K".into(), "echo pty-ready-1b2c".into()],
            env_overrides: vec![("PTY_TEST_VAR".into(), "1".into())],
            cwd: None,
            rows: 24,
            cols: 80,
        };
        let id = spawn_pty("test-interactive", cfg, tx).expect("PTY 应能创建");

        // 等初始化（ConPTY 启动握手 ESC[6n 等）后写入命令
        std::thread::sleep(Duration::from_millis(800));
        {
            let mut map = SESSIONS.lock().unwrap();
            let s = map.as_mut().unwrap().get_mut(&id).unwrap();
            s.writer
                .write_all(b"echo pty-hello-9a3f\r\n")
                .expect("写入命令");
            s.writer.flush().ok();
        }

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut acc = String::new();
        let mut replied_dsr = false;
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(chunk) => {
                    let text = String::from_utf8_lossy(&chunk).to_string();
                    // 模拟终端应答 ConPTY 启动时的光标查询（真实场景由 xterm.js 自动应答）
                    if !replied_dsr && text.contains("\u{1b}[6n") {
                        replied_dsr = true;
                        pty_write(id, "\x1b[1;1R").ok();
                    }
                    acc.push_str(&text);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if acc.contains("pty-hello-9a3f") {
                break;
            }
        }
        let _ = pty_close(id);
        assert!(
            acc.contains("pty-hello-9a3f"),
            "15 秒内未读到回显，实际输出: {acc:?}"
        );
    }

    /// 会话保持语义：attach 按服务找到既有会话并回放缓冲（离开页面不销毁）
    #[test]
    fn attach按服务恢复会话与回放缓冲() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let cfg = PtyConfig {
            program: "cmd.exe".into(),
            args: vec!["/K".into(), "echo attach-marker-77f0".into()],
            env_overrides: vec![],
            cwd: None,
            rows: 24,
            cols: 80,
        };
        let id = spawn_pty("test-attach-svc", cfg, tx).expect("PTY 应能创建");
        // 与交互测试一致：等初始化后写入命令，促使 conhost 进入持续输出
        std::thread::sleep(Duration::from_millis(800));
        {
            let mut map = SESSIONS.lock().unwrap();
            let s = map.as_mut().unwrap().get_mut(&id).unwrap();
            s.writer
                .write_all(b"echo attach-marker-77f0\r\n")
                .expect("写入命令");
            s.writer.flush().ok();
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut saw_output = false;
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(chunk) => {
                    saw_output = true;
                    // 模拟终端应答 DSR，促使 conhost 持续输出
                    if String::from_utf8_lossy(&chunk).contains("\u{1b}[6n") {
                        pty_write(id, "\x1b[1;1R").ok();
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(_) => {}
            }
        }
        assert!(saw_output, "10+15 秒内未收到任何 PTY 输出");
        let (aid, backlog) = attach_by_service("test-attach-svc").expect("离开页面后应能 attach 到既有会话");
        assert_eq!(aid, id);
        assert!(
            backlog.contains("attach-marker-77f0"),
            "回放缓冲应含输出，实际: {} 字节",
            backlog.len()
        );
        // 其他服务无会话
        assert!(attach_by_service("no-such-service").is_none());
        let _ = pty_close(id);
        // 关闭后 attach 不应再找到
        assert!(attach_by_service("test-attach-svc").is_none());
    }

    #[test]
    fn 写入与关闭对不存在会话安全() {
        assert!(pty_write(u64::MAX, "x").is_err());
        assert!(pty_resize(u64::MAX, 10, 10).is_err());
        assert!(pty_close(u64::MAX).is_ok()); // 幂等
    }
}
