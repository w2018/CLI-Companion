//! 内置 FTP 服务端（v2.6.0，应用功能页）
//!
//! - tokio 手写 RFC 959 子集，零新依赖；多监听器（多端口 + 各自根目录）
//! - 用户全局共享，按用户细粒度权限（list/download/upload/delete/rename/mkdir）
//! - 用户多目录授权：登录监听器要求其根目录在授权列表内；其余授权目录以
//!   虚拟子目录挂载进会话根视图（挂载名冲突自动加后缀，监狱内/上级目录跳过）
//! - 安全：字典式路径监狱（组件级校验后拼接到已规范化根）；挂载根禁删/禁改名；
//!   PORT 主动模式仅允许连接控制连接对端 IP；登录失败 3 次断开 + 1s 延迟；
//!   密码常数时间比较；控制连接上限 64；空闲 300s 超时
//! - 生命周期：监督任务订阅 config.changed，按"站点指纹"（enabled+被动区间+
//!   监听器列表）变化自动启停/换端口；用户/权限修改对新登录即时生效

use crate::app_config::{normalize_path_key, FtpPermissions, FtpSettings, FtpUser};
use crate::state::AppState;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// 控制连接空闲超时
const CONTROL_IDLE: Duration = Duration::from_secs(300);
/// 数据连接等待（被动 accept）/ 主动连接超时
const DATA_WAIT: Duration = Duration::from_secs(30);
const CONNECT_WAIT: Duration = Duration::from_secs(10);
/// 单条命令行最大字节数
const MAX_LINE: usize = 4096;
/// 控制连接上限（含未登录）
const MAX_CONNECTIONS: usize = 64;
/// 连续登录失败上限
const MAX_AUTH_FAILS: u32 = 3;

// ===== 共享运行时状态（RPC ftp.status 读取） =====

/// 单个服务端实例的会话计数（每次启停更换实例）
#[derive(Default)]
pub struct FtpServerShared {
    /// 已通过认证的会话数
    pub sessions: AtomicUsize,
    /// 控制连接数（含未登录）
    pub connections: AtomicUsize,
}

/// FTP 运行时快照（监督任务写入，RPC 读取）
pub struct FtpRuntime {
    running: AtomicBool,
    starts: AtomicUsize,
    ports: Mutex<Vec<u16>>,
    server: Mutex<Option<Arc<FtpServerShared>>>,
    last_error: Mutex<Option<String>>,
    local_ip: Mutex<Option<String>>,
}

static RUNTIME: OnceLock<FtpRuntime> = OnceLock::new();

fn runtime() -> &'static FtpRuntime {
    RUNTIME.get_or_init(|| FtpRuntime {
        running: AtomicBool::new(false),
        starts: AtomicUsize::new(0),
        ports: Mutex::new(Vec::new()),
        server: Mutex::new(None),
        last_error: Mutex::new(None),
        local_ip: Mutex::new(None),
    })
}

/// ftp.status 用运行时快照
#[derive(Debug, Clone)]
pub struct FtpRuntimeSnapshot {
    pub running: bool,
    pub starts: usize,
    pub ports: Vec<u16>,
    pub sessions: usize,
    pub last_error: Option<String>,
    pub local_ip: Option<String>,
}

pub fn runtime_snapshot() -> FtpRuntimeSnapshot {
    let rt = runtime();
    FtpRuntimeSnapshot {
        running: rt.running.load(Ordering::Relaxed),
        starts: rt.starts.load(Ordering::Relaxed),
        ports: rt.ports.lock().unwrap().clone(),
        sessions: rt
            .server
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.sessions.load(Ordering::Relaxed))
            .unwrap_or(0),
        last_error: rt.last_error.lock().unwrap().clone(),
        local_ip: rt.local_ip.lock().unwrap().clone(),
    }
}

fn set_runtime(
    running: bool,
    ports: Vec<u16>,
    server: Option<Arc<FtpServerShared>>,
    error: Option<String>,
) {
    let rt = runtime();
    rt.running.store(running, Ordering::Relaxed);
    if running {
        rt.starts.fetch_add(1, Ordering::Relaxed);
    }
    *rt.ports.lock().unwrap() = ports;
    *rt.server.lock().unwrap() = server;
    *rt.last_error.lock().unwrap() = error;
}

/// 探测本机局域网 IP（UDP connect 不发包，仅取路由出口地址）
fn detect_local_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

// ===== 监督任务 =====

/// 站点指纹：仅包含"需要重启服务端"的配置项（用户/权限修改不触发重启）
fn fingerprint(ftp: &FtpSettings) -> String {
    let listeners: Vec<_> = ftp
        .listeners
        .iter()
        .map(|l| {
            serde_json::json!({
                "name": l.name,
                "port": l.port,
                "root": normalize_path_key(&l.root),
                "enabled": l.enabled,
            })
        })
        .collect();
    serde_json::json!({
        "enabled": ftp.enabled,
        "passive": [ftp.passive_port_start, ftp.passive_port_end],
        "listeners": listeners,
    })
    .to_string()
}

/// 监督任务：按最新 app 配置启停 FTP 服务端（随 config.changed 即时生效）
pub fn spawn_supervisor(state: AppState) {
    tokio::spawn(async move {
        let mut rx = state.events.subscribe();
        // 当前运行中的站点指纹与停止信号
        let mut running_fp: Option<String> = None;
        let mut shutdown: Option<Arc<Notify>> = None;
        loop {
            let ftp = state.app().await.ftp;
            let want = if ftp.enabled && ftp.validate().is_ok() {
                Some(fingerprint(&ftp))
            } else {
                None
            };
            match (&want, &running_fp) {
                (Some(fp), Some(cur)) if fp == cur => {}
                (Some(fp), _) => {
                    if let Some(sd) = shutdown.take() {
                        sd.notify_waiters();
                    }
                    running_fp = None;
                    match start_server(&state, &ftp).await {
                        Ok((sd, shared, ports)) => {
                            set_runtime(true, ports, Some(shared), None);
                            *runtime().local_ip.lock().unwrap() = detect_local_ip();
                            running_fp = Some(fp.clone());
                            shutdown = Some(sd);
                        }
                        Err(e) => {
                            tracing::warn!("FTP 服务端启动失败: {e}");
                            set_runtime(false, Vec::new(), None, Some(e));
                        }
                    }
                }
                (None, Some(_)) => {
                    if let Some(sd) = shutdown.take() {
                        sd.notify_waiters();
                    }
                    running_fp = None;
                    set_runtime(false, Vec::new(), None, None);
                }
                (None, None) => {}
            }
            // 等下一次配置变更或 daemon 关闭
            tokio::select! {
                ev = rx.recv() => match ev {
                    Ok(e) if e.topic == cli_companion_protocol::EventTopic::ConfigChanged => continue,
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break, // 总线关闭（daemon 退出）
                },
                _ = state.shutdown.notified() => {
                    if let Some(sd) = shutdown.take() {
                        sd.notify_waiters();
                    }
                    set_runtime(false, Vec::new(), None, None);
                    break;
                }
            }
        }
    });
}

/// 绑定端口（重启场景旧监听器尚未释放：短暂重试）
async fn bind_retry(port: u16) -> io::Result<TcpListener> {
    let mut last = None;
    for _ in 0..40 {
        match TcpListener::bind(("0.0.0.0", port)).await {
            Ok(l) => return Ok(l),
            Err(e) => {
                last = Some(e);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("bind failed")))
}

/// 拉起服务端：绑定全部启用的监听器（根目录不存在则创建）
async fn start_server(
    state: &AppState,
    ftp: &FtpSettings,
) -> Result<(Arc<Notify>, Arc<FtpServerShared>, Vec<u16>), String> {
    let mut bound = Vec::new();
    let mut ports = Vec::new();
    for l in ftp.listeners.iter().filter(|l| l.enabled) {
        let _ = std::fs::create_dir_all(&l.root);
        let root = std::fs::canonicalize(&l.root)
            .map_err(|e| format!("监听器 {} 根目录不可用 ({}): {e}", l.name, l.root.display()))?;
        let listener = bind_retry(l.port)
            .await
            .map_err(|e| format!("监听端口 {} 绑定失败: {e}", l.port))?;
        let actual = listener.local_addr().map(|a| a.port()).unwrap_or(l.port);
        ports.push(actual);
        bound.push(BoundListener {
            name: l.name.clone(),
            root,
            listener,
        });
    }
    let shared = Arc::new(FtpServerShared::default());
    let sd = Arc::new(Notify::new());
    tokio::spawn(run_server_on(
        state.clone(),
        bound,
        (ftp.passive_port_start, ftp.passive_port_end),
        shared.clone(),
        sd.clone(),
    ));
    tracing::info!(?ports, "FTP 服务端已启动");
    Ok((sd, shared, ports))
}

// ===== 服务端主体 =====

/// 已绑定并规范化根目录的监听器
pub struct BoundListener {
    pub name: String,
    /// 规范化后的根目录（监狱根）
    pub root: PathBuf,
    pub listener: TcpListener,
}

/// 在给定监听器集合上运行服务端（测试直接注入 127.0.0.1:0 监听器）
pub async fn run_server_on(
    state: AppState,
    listeners: Vec<BoundListener>,
    passive: (u16, u16),
    shared: Arc<FtpServerShared>,
    shutdown: Arc<Notify>,
) -> io::Result<()> {
    for bl in listeners {
        let st = state.clone();
        let sh = shared.clone();
        let sd = shutdown.clone();
        tokio::spawn(async move {
            accept_loop(st, bl, passive, sh, sd).await;
        });
    }
    shutdown.notified().await;
    tracing::info!("FTP 服务端已停止");
    Ok(())
}

/// 单监听器接受循环
async fn accept_loop(
    state: AppState,
    bl: BoundListener,
    passive: (u16, u16),
    shared: Arc<FtpServerShared>,
    shutdown: Arc<Notify>,
) {
    loop {
        let accepted = tokio::select! {
            r = bl.listener.accept() => r,
            _ = shutdown.notified() => return,
        };
        let (stream, peer) = match accepted {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!("FTP 接受连接失败: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        if shared.connections.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
            tracing::debug!(%peer, "FTP 连接数达上限，拒绝");
            let _ = limit_reply(stream).await;
            continue;
        }
        let st = state.clone();
        let sh = shared.clone();
        let sd = shutdown.clone();
        let site = SiteCfg {
            name: bl.name.clone(),
            root: bl.root.clone(),
            passive,
        };
        tokio::spawn(handle_session(st, stream, peer, site, sh, sd));
    }
}

/// 超限时直接回复 421 后关闭
async fn limit_reply(mut stream: TcpStream) -> io::Result<()> {
    stream
        .write_all(b"421 Too many connections, try later.\r\n")
        .await
        .ok();
    stream.shutdown().await.ok();
    Ok(())
}

/// 会话所属站点配置
struct SiteCfg {
    #[allow(dead_code)]
    name: String,
    /// 规范化监狱根
    root: PathBuf,
    passive: (u16, u16),
}

async fn handle_session(
    state: AppState,
    stream: TcpStream,
    peer: SocketAddr,
    site: SiteCfg,
    shared: Arc<FtpServerShared>,
    shutdown: Arc<Notify>,
) {
    let local_ip = stream
        .local_addr()
        .map(|a| a.ip())
        .unwrap_or(IpAddr::from([127, 0, 0, 1]));
    let mut sess = Session {
        stream,
        peer,
        local_ip,
        site,
        state,
        shared,
        shutdown,
        rbuf: Vec::new(),
        user: None,
        auth_user_pending: None,
        auth_fails: 0,
        cur: VPath::root(),
        pending: None,
        rnfr: None,
        rest_offset: 0,
        _conn_guard: None,
        _auth_guard: None,
    };
    sess.run().await;
}

// ===== 连接/会话计数守卫 =====

struct ConnGuard {
    shared: Arc<FtpServerShared>,
}
impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.shared.connections.fetch_sub(1, Ordering::Relaxed);
    }
}

struct AuthGuard {
    shared: Arc<FtpServerShared>,
}
impl Drop for AuthGuard {
    fn drop(&mut self) {
        self.shared.sessions.fetch_sub(1, Ordering::Relaxed);
    }
}

// ===== 虚拟目录视图 =====

/// 授权目录挂载项
struct Mount {
    /// 虚拟挂载名（根视图中的条目名）
    vname: String,
    /// 规范化真实根
    root: PathBuf,
}

/// 会话目录视图：监狱根 + 授权目录挂载
struct View {
    home: PathBuf,
    mounts: Vec<Mount>,
}

/// 虚拟路径：mount=None+comps 空 = 虚拟根 "/"
#[derive(Clone)]
struct VPath {
    /// Some(idx) = 位于挂载 idx 内
    mount: Option<usize>,
    /// 监狱内相对组件（home 或 mount 根之下）
    comps: Vec<String>,
    /// 展示用虚拟组件（进入挂载时首项为挂载名）
    vcomps: Vec<String>,
}

impl VPath {
    fn root() -> Self {
        Self {
            mount: None,
            comps: Vec::new(),
            vcomps: Vec::new(),
        }
    }
    fn is_root(&self) -> bool {
        self.mount.is_none() && self.comps.is_empty()
    }
    /// 是否正好位于挂载目录本身（禁删/禁改名）
    fn at_mount_root(&self) -> bool {
        self.mount.is_some() && self.comps.is_empty()
    }
    /// PWD 展示路径
    fn display(&self) -> String {
        if self.vcomps.is_empty() {
            "/".into()
        } else {
            format!("/{}", self.vcomps.join("/"))
        }
    }
    /// 真实文件系统路径
    fn real(&self, view: &View) -> PathBuf {
        let base = match self.mount {
            None => view.home.clone(),
            Some(i) => view.mounts[i].root.clone(),
        };
        let mut p = base;
        for c in &self.comps {
            p.push(c);
        }
        p
    }
}

/// 路径组件合法性（字典式监狱的核心：拒绝一切可用于越界的形态）
fn validate_seg(seg: &str) -> Result<(), String> {
    if seg.is_empty() || seg == "." || seg == ".." {
        return Err("非法路径组件".into());
    }
    if seg.len() > 255 {
        return Err("名称过长".into());
    }
    for c in seg.chars() {
        if matches!(c, '\\' | ':' | '<' | '>' | '|' | '?' | '*') || c.is_control() {
            return Err(format!("路径包含非法字符: {c:?}"));
        }
    }
    Ok(())
}

/// 从当前虚拟路径解析目标（绝对路径从虚拟根出发，相对路径从当前出发）
/// CWD/CDUP 语义：`..` 在根处钳制（不出监狱）
fn resolve(view: &View, cur: &VPath, target: &str) -> Result<VPath, String> {
    let mut p = if target.starts_with('/') {
        VPath::root()
    } else {
        cur.clone()
    };
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                // 已在该基根（监狱根或挂载根）顶部 → 回虚拟根（挂载根上跳必须退出挂载）
                if p.comps.pop().is_none() {
                    p.mount = None;
                    p.vcomps.clear();
                } else {
                    p.vcomps.pop();
                }
            }
            _ => {
                validate_seg(seg)?;
                // 虚拟根处挂载名优先（构建时已避开与根内条目重名）
                if p.mount.is_none() && p.comps.is_empty() {
                    if let Some(idx) = view
                        .mounts
                        .iter()
                        .position(|m| m.vname.to_lowercase() == seg.to_lowercase())
                    {
                        p = VPath {
                            mount: Some(idx),
                            comps: Vec::new(),
                            vcomps: vec![view.mounts[idx].vname.clone()],
                        };
                        continue;
                    }
                }
                p.comps.push(seg.to_string());
                p.vcomps.push(seg.to_string());
            }
        }
    }
    Ok(p)
}

/// 文件操作解析（MKD/RMD/DELE/RNFR/RNTO/STOR/APPE/RETR/SIZE/MDTM/LIST）：
/// 路径中任何 `..` 组件一律拒绝——写操作绝不接受上跳语义，
/// 读操作也不需要（客户端都能改写为规范路径）
fn resolve_fs(view: &View, cur: &VPath, target: &str) -> Result<VPath, String> {
    if target
        .split('/')
        .any(|seg| seg == ".." || seg.starts_with("..") || seg.ends_with(".."))
    {
        return Err("路径不允许包含 ..".into());
    }
    resolve(view, cur, target)
}

/// 用户是否被授权访问站点根（配置路径或规范化路径一致即认可）
fn user_allows_listener(user: &FtpUser, site_root_canon: &Path) -> bool {
    let ck = normalize_path_key(site_root_canon);
    if ck.is_empty() {
        return false;
    }
    user.allowed_roots.iter().any(|p| {
        normalize_path_key(p) == ck
            || std::fs::canonicalize(p)
                .map(|c| normalize_path_key(&c) == ck)
                .unwrap_or(false)
    })
}

/// 构建会话视图：监狱根 + 其余授权目录挂载
fn build_view(site_root_canon: &Path, user: &FtpUser) -> View {
    let home = site_root_canon.to_path_buf();
    let hk = normalize_path_key(&home);
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Ok(rd) = std::fs::read_dir(&home) {
        for e in rd.flatten() {
            used.insert(e.file_name().to_string_lossy().to_lowercase());
        }
    }
    let mut mounts = Vec::new();
    let mut seen_mount_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    for ar in &user.allowed_roots {
        let Ok(c) = std::fs::canonicalize(ar) else {
            continue; // 不存在的授权目录跳过
        };
        if !c.is_dir() {
            continue;
        }
        let nk = normalize_path_key(&c);
        if nk == hk
            || nk.starts_with(&format!("{hk}\\"))
            // 授权目录是监狱的上级：挂载等于扩监狱，拒绝
            || hk.starts_with(&format!("{nk}\\"))
            || !seen_mount_keys.insert(nk)
        {
            continue;
        }
        let Some(base) = c.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue; // 无末段（如盘根）无法取挂载名
        };
        let mut vname = base.clone();
        let mut n = 2u32;
        while used.contains(&vname.to_lowercase()) {
            if n > 9 {
                vname.clear();
                break;
            }
            vname = format!("{base}-{n}");
            n += 1;
        }
        if vname.is_empty() {
            continue;
        }
        used.insert(vname.to_lowercase());
        mounts.push(Mount { vname, root: c });
    }
    View { home, mounts }
}

/// 常数时间字符串比较
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

// ===== 目录列表 =====

struct EntryInfo {
    name: String,
    is_dir: bool,
    size: u64,
    mtime: Option<std::time::SystemTime>,
}

/// 收集目标（虚拟根含挂载项；挂载名遮蔽根内同名真实条目）
fn collect_entries(view: &View, vp: &VPath) -> io::Result<Vec<EntryInfo>> {
    let real = vp.real(view);
    let md = std::fs::metadata(&real);
    let is_dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
    if !vp.is_root() && !is_dir {
        // 单文件：LIST /file.txt 返回单行
        if let Ok(m) = md {
            return Ok(vec![EntryInfo {
                name: vp.vcomps.last().cloned().unwrap_or_default(),
                is_dir: false,
                size: m.len(),
                mtime: m.modified().ok(),
            }]);
        }
    }
    let mut out = Vec::new();
    let mount_names: std::collections::HashSet<String> = if vp.is_root() {
        view.mounts.iter().map(|m| m.vname.to_lowercase()).collect()
    } else {
        Default::default()
    };
    for e in std::fs::read_dir(&real)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_string();
        if mount_names.contains(&name.to_lowercase()) {
            continue;
        }
        let m = e.metadata()?;
        out.push(EntryInfo {
            name,
            is_dir: m.is_dir(),
            size: m.len(),
            mtime: m.modified().ok(),
        });
    }
    if vp.is_root() {
        for m in &view.mounts {
            let md = std::fs::metadata(&m.root).ok();
            out.push(EntryInfo {
                name: m.vname.clone(),
                is_dir: true,
                size: 0,
                mtime: md.as_ref().and_then(|x| x.modified().ok()),
            });
        }
    }
    Ok(out)
}

/// LIST 单行（unix ls -l 风格，兼容主流客户端）
fn list_line(e: &EntryInfo) -> String {
    let t = if e.is_dir { 'd' } else { '-' };
    let mtime = e
        .mtime
        .map(chrono::DateTime::<chrono::Local>::from)
        .unwrap_or_else(chrono::Local::now);
    let now = chrono::Local::now();
    let date = if mtime > now - chrono::Duration::days(182) {
        mtime.format("%b %e %H:%M").to_string()
    } else {
        mtime.format("%b %e  %Y").to_string()
    };
    format!(
        "{t}rw-r--r-- 1 ftp ftp {size:>13} {date} {name}",
        size = e.size,
        name = e.name
    )
}

/// MLSD/MLST 事实行（UTC 修改时间）
fn mlsd_facts(e: &EntryInfo) -> String {
    let modify = e
        .mtime
        .map(chrono::DateTime::<chrono::Utc>::from)
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y%m%d%H%M%S");
    format!(
        "type={};size={};modify={};",
        if e.is_dir { "dir" } else { "file" },
        e.size,
        modify
    )
}

// ===== 会话 =====

enum DataMode {
    Passive(TcpListener),
    Active(SocketAddr),
}

struct AuthedUser {
    #[allow(dead_code)] // 保留用户名供后续会话审计/日志使用
    name: String,
    perms: FtpPermissions,
    view: View,
}

struct Session {
    stream: TcpStream,
    peer: SocketAddr,
    local_ip: IpAddr,
    site: SiteCfg,
    state: AppState,
    shared: Arc<FtpServerShared>,
    shutdown: Arc<Notify>,
    rbuf: Vec<u8>,
    user: Option<AuthedUser>,
    auth_user_pending: Option<String>,
    auth_fails: u32,
    cur: VPath,
    pending: Option<DataMode>,
    /// RNFR 暂存（与数据连接模式互不影响）
    rnfr: Option<VPath>,
    rest_offset: u64,
    _conn_guard: Option<ConnGuard>,
    _auth_guard: Option<AuthGuard>,
}

impl Session {
    async fn reply(&mut self, text: &str) -> io::Result<()> {
        self.stream
            .write_all(format!("{text}\r\n").as_bytes())
            .await?;
        self.stream.flush().await
    }

    async fn run(&mut self) {
        self._conn_guard = Some(ConnGuard {
            shared: self.shared.clone(),
        });
        self.shared.connections.fetch_add(1, Ordering::Relaxed);
        if self
            .reply("220 CLI Companion FTP server ready.")
            .await
            .is_err()
        {
            return;
        }
        loop {
            let line = tokio::select! {
                r = read_line(&mut self.stream, &mut self.rbuf) => r,
                _ = self.shutdown.notified() => {
                    let _ = self.reply("421 Daemon shutting down.").await;
                    return;
                }
            };
            match line {
                Err(e) => {
                    let _ = self.reply(&format!("421 {e}")).await;
                    return;
                }
                Ok(None) => return, // 客户端断开
                Ok(Some(l)) => {
                    if self.dispatch(&l).await.is_err() {
                        return;
                    }
                }
            }
        }
    }

    /// 分发单条命令；返回 Err 表示关闭连接
    async fn dispatch(&mut self, line: &str) -> Result<(), ()> {
        let (cmd, arg) = match line.split_once(' ') {
            Some((c, a)) => (c.to_uppercase(), a.trim()),
            None => (line.to_uppercase(), ""),
        };
        // 登录前允许的命令
        if self.user.is_none()
            && !matches!(
                cmd.as_str(),
                "USER" | "PASS" | "QUIT" | "NOOP" | "SYST" | "FEAT" | "HELP" | "OPTS" | "TYPE"
            )
        {
            self.reply("530 Please log in with USER and PASS.")
                .await
                .map_err(|_| ())?;
            return Ok(());
        }
        match cmd.as_str() {
            "QUIT" => {
                let _ = self.reply("221 Goodbye.").await;
                Err(())
            }
            "NOOP" => self.reply("200 NOOP ok.").await.map_err(|_| ()),
            "SYST" => self.reply("215 UNIX Type: L8").await.map_err(|_| ()),
            "HELP" => self
                .reply("214 Commands: USER PASS QUIT NOOP SYST FEAT OPTS TYPE PWD CWD CDUP MKD RMD DELE RNFR RNTO SIZE MDTM LIST NLST MLSD MLST RETR STOR APPE REST PASV EPSV PORT EPRT ABOR.")
                .await
                .map_err(|_| ()),
            "FEAT" => {
                self.reply("211-Features:").await.map_err(|_| ())?;
                for f in [" UTF8", " MLST type*;size*;modify*;", " SIZE", " MDTM", " REST STREAM", " EPSV", " TVFS"] {
                    self.reply(f).await.map_err(|_| ())?;
                }
                self.reply("211 End").await.map_err(|_| ())
            }
            "OPTS" => {
                if arg.eq_ignore_ascii_case("UTF8 ON") {
                    self.reply("200 UTF8 mode enabled.").await.map_err(|_| ())
                } else {
                    self.reply("501 Option not understood.").await.map_err(|_| ())
                }
            }
            "TYPE" => match arg.chars().next().map(|c| c.to_ascii_uppercase()) {
                Some('I') | Some('A') | Some('L') => {
                    self.reply("200 Type set.").await.map_err(|_| ())
                }
                _ => self.reply("504 Type not supported.").await.map_err(|_| ()),
            },
            "STRU" | "MODE" => self.reply("502 Not implemented.").await.map_err(|_| ()),
            "USER" => self.cmd_user(arg).await,
            "PASS" => self.cmd_pass(arg).await,
            "PWD" | "XPWD" => {
                self.reply(&format!("257 \"{}\" is the current directory", self.cur.display()))
                    .await
                    .map_err(|_| ())
            }
            "CWD" => self.cmd_cwd(arg).await,
            "CDUP" => self.cmd_cwd("..").await,
            "MKD" | "XMKD" => self.cmd_mkd(arg).await,
            "RMD" | "XRMD" => self.cmd_rmd(arg).await,
            "DELE" => self.cmd_dele(arg).await,
            "RNFR" => self.cmd_rnfr(arg).await,
            "RNTO" => self.cmd_rnto(arg).await,
            "SIZE" => self.cmd_size(arg).await,
            "MDTM" => self.cmd_mdtm(arg).await,
            "REST" => {
                match arg.parse::<u64>() {
                    Ok(n) => {
                        self.rest_offset = n;
                        self.reply(&format!("350 Restarting at {n}. Send RETR to resume."))
                            .await
                            .map_err(|_| ())
                    }
                    Err(_) => self.reply("501 Invalid REST offset.").await.map_err(|_| ()),
                }
            }
            "PASV" => self.cmd_pasv().await,
            "EPSV" => self.cmd_epsv(arg).await,
            "PORT" => self.cmd_port(arg).await,
            "EPRT" => self.cmd_eprt(arg).await,
            "LIST" => self.cmd_list(strip_list_flags(arg), false).await,
            "NLST" => self.cmd_list(strip_list_flags(arg), true).await,
            "MLSD" => self.cmd_mlsd(arg).await,
            "MLST" => self.cmd_mlst(arg).await,
            "RETR" => self.cmd_retr(arg).await,
            "STOR" => self.cmd_stor(arg, false).await,
            "APPE" => self.cmd_stor(arg, true).await,
            "ABOR" => self.reply("226 No transfer in progress.").await.map_err(|_| ()),
            "ALLO" | "ACCT" | "SMNT" | "STOU" | "SITE" => {
                self.reply("202 Command not implemented, superfluous.").await.map_err(|_| ())
            }
            _ => self.reply("500 Command not understood.").await.map_err(|_| ()),
        }
    }

    // ===== 认证 =====

    async fn cmd_user(&mut self, arg: &str) -> Result<(), ()> {
        if arg.is_empty() {
            return self.reply("501 Username required.").await.map_err(|_| ());
        }
        // 重新登录：清除现有认证
        if self.user.take().is_some() {
            self._auth_guard = None;
        }
        self.auth_user_pending = Some(arg.to_string());
        self.reply("331 Password required.").await.map_err(|_| ())
    }

    async fn cmd_pass(&mut self, arg: &str) -> Result<(), ()> {
        let Some(username) = self.auth_user_pending.take() else {
            return self
                .reply("503 Login with USER first.")
                .await
                .map_err(|_| ());
        };
        // 登录时刻读取最新用户配置与密码（权限/授权修改对新登录即时生效）
        let app = self.state.app().await;
        let user = app
            .ftp
            .users
            .iter()
            .find(|u| u.enabled && u.username.eq_ignore_ascii_case(&username))
            .cloned();
        let stored = self
            .state
            .config
            .lock()
            .await
            .secrets
            .ftp_password(&username);
        let ok = match (&user, &stored) {
            (Some(u), Some(pw)) => ct_eq(pw, arg) && user_allows_listener(u, &self.site.root),
            _ => false,
        };
        if !ok {
            self.auth_fails += 1;
            if self.auth_fails >= MAX_AUTH_FAILS {
                let _ = self.reply("530 Login incorrect.").await;
                let _ = self.reply("421 Too many failed logins.").await;
                return Err(());
            }
            tokio::time::sleep(Duration::from_secs(1)).await; // 拖慢爆破
            return self.reply("530 Login incorrect.").await.map_err(|_| ());
        }
        let user = user.unwrap();
        let view = build_view(&self.site.root, &user);
        tracing::info!(%username, peer = %self.peer, site = %self.site.name, "FTP 登录成功");
        self.auth_fails = 0;
        self.user = Some(AuthedUser {
            name: user.username.clone(),
            perms: user.permissions,
            view,
        });
        self._auth_guard = Some(AuthGuard {
            shared: self.shared.clone(),
        });
        self.shared.sessions.fetch_add(1, Ordering::Relaxed);
        self.reply("230 Login successful.").await.map_err(|_| ())
    }

    fn perms(&self) -> &FtpPermissions {
        &self.user.as_ref().unwrap().perms
    }

    fn view(&self) -> &View {
        &self.user.as_ref().unwrap().view
    }

    // ===== 目录与文件操作 =====

    async fn cmd_cwd(&mut self, arg: &str) -> Result<(), ()> {
        if !self.perms().list {
            return self
                .reply("550 权限不足：未授权浏览目录.")
                .await
                .map_err(|_| ());
        }
        if arg.is_empty() {
            return self.reply("501 Path required.").await.map_err(|_| ());
        }
        let vp = match resolve(self.view(), &self.cur, arg) {
            Ok(v) => v,
            Err(e) => return self.reply(&format!("550 {e}")).await.map_err(|_| ()),
        };
        match std::fs::metadata(vp.real(self.view())) {
            Ok(m) if m.is_dir() => {
                self.cur = vp;
                self.reply("250 Directory changed.").await.map_err(|_| ())
            }
            _ => self.reply("550 Directory not found.").await.map_err(|_| ()),
        }
    }

    async fn cmd_mkd(&mut self, arg: &str) -> Result<(), ()> {
        if !self.perms().mkdir {
            return self
                .reply("550 权限不足：未授权创建目录.")
                .await
                .map_err(|_| ());
        }
        let Ok(vp) = resolve_fs(self.view(), &self.cur, arg) else {
            return self.reply("550 非法路径.").await.map_err(|_| ());
        };
        if vp.is_root() || vp.at_mount_root() {
            return self.reply("550 目录已存在.").await.map_err(|_| ());
        }
        match std::fs::create_dir(vp.real(self.view())) {
            Ok(()) => self
                .reply(&format!("257 \"{}\" created.", vp.display()))
                .await
                .map_err(|_| ()),
            Err(_) => self
                .reply("550 Create directory failed.")
                .await
                .map_err(|_| ()),
        }
    }

    async fn cmd_rmd(&mut self, arg: &str) -> Result<(), ()> {
        if !self.perms().delete {
            return self
                .reply("550 权限不足：未授权删除.")
                .await
                .map_err(|_| ());
        }
        let Ok(vp) = resolve_fs(self.view(), &self.cur, arg) else {
            return self.reply("550 非法路径.").await.map_err(|_| ());
        };
        if vp.is_root() {
            return self.reply("550 不能删除根目录.").await.map_err(|_| ());
        }
        if vp.at_mount_root() {
            return self.reply("550 不能删除挂载目录.").await.map_err(|_| ());
        }
        match std::fs::remove_dir(vp.real(self.view())) {
            Ok(()) => self.reply("250 Directory removed.").await.map_err(|_| ()),
            Err(_) => self
                .reply("550 Remove directory failed.")
                .await
                .map_err(|_| ()),
        }
    }

    async fn cmd_dele(&mut self, arg: &str) -> Result<(), ()> {
        if !self.perms().delete {
            return self
                .reply("550 权限不足：未授权删除.")
                .await
                .map_err(|_| ());
        }
        let Ok(vp) = resolve_fs(self.view(), &self.cur, arg) else {
            return self.reply("550 非法路径.").await.map_err(|_| ());
        };
        if vp.is_root() || vp.at_mount_root() {
            return self.reply("550 不能删除目录项.").await.map_err(|_| ());
        }
        let real = vp.real(self.view());
        match std::fs::metadata(&real) {
            Ok(m) if m.is_file() => {}
            _ => return self.reply("550 File not found.").await.map_err(|_| ()),
        }
        match std::fs::remove_file(&real) {
            Ok(()) => self.reply("250 File deleted.").await.map_err(|_| ()),
            Err(_) => self.reply("550 Delete failed.").await.map_err(|_| ()),
        }
    }

    async fn cmd_rnfr(&mut self, arg: &str) -> Result<(), ()> {
        if !self.perms().rename {
            return self
                .reply("550 权限不足：未授权重命名.")
                .await
                .map_err(|_| ());
        }
        let Ok(vp) = resolve_fs(self.view(), &self.cur, arg) else {
            return self.reply("550 非法路径.").await.map_err(|_| ());
        };
        if vp.is_root() || vp.at_mount_root() {
            return self.reply("550 不能重命名该目录.").await.map_err(|_| ());
        }
        if !vp.real(self.view()).exists() {
            return self.reply("550 Source not found.").await.map_err(|_| ());
        }
        self.rnfr = Some(vp);
        self.reply("350 Ready for RNTO.").await.map_err(|_| ())
    }

    async fn cmd_rnto(&mut self, arg: &str) -> Result<(), ()> {
        if !self.perms().rename {
            return self
                .reply("550 权限不足：未授权重命名.")
                .await
                .map_err(|_| ());
        }
        let Some(src) = self.rnfr.take() else {
            return self.reply("503 Use RNFR first.").await.map_err(|_| ());
        };
        let Ok(dst) = resolve_fs(self.view(), &self.cur, arg) else {
            return self.reply("550 非法路径.").await.map_err(|_| ());
        };
        if dst.is_root() || dst.at_mount_root() {
            return self.reply("550 目标路径不合法.").await.map_err(|_| ());
        }
        match std::fs::rename(src.real(self.view()), dst.real(self.view())) {
            Ok(()) => self.reply("250 Rename successful.").await.map_err(|_| ()),
            Err(_) => self.reply("550 Rename failed.").await.map_err(|_| ()),
        }
    }

    async fn cmd_size(&mut self, arg: &str) -> Result<(), ()> {
        if !self.perms().download {
            return self
                .reply("550 权限不足：未授权下载.")
                .await
                .map_err(|_| ());
        }
        let Ok(vp) = resolve_fs(self.view(), &self.cur, arg) else {
            return self.reply("550 非法路径.").await.map_err(|_| ());
        };
        match std::fs::metadata(vp.real(self.view())) {
            Ok(m) if m.is_file() => self
                .reply(&format!("213 {}", m.len()))
                .await
                .map_err(|_| ()),
            _ => self.reply("550 File not found.").await.map_err(|_| ()),
        }
    }

    async fn cmd_mdtm(&mut self, arg: &str) -> Result<(), ()> {
        if !self.perms().download {
            return self
                .reply("550 权限不足：未授权下载.")
                .await
                .map_err(|_| ());
        }
        let Ok(vp) = resolve_fs(self.view(), &self.cur, arg) else {
            return self.reply("550 非法路径.").await.map_err(|_| ());
        };
        match std::fs::metadata(vp.real(self.view())).and_then(|m| m.modified()) {
            Ok(t) => {
                let s = chrono::DateTime::<chrono::Utc>::from(t).format("%Y%m%d%H%M%S");
                self.reply(&format!("213 {s}")).await.map_err(|_| ())
            }
            Err(_) => self.reply("550 File not found.").await.map_err(|_| ()),
        }
    }

    // ===== 数据连接模式 =====

    async fn cmd_pasv(&mut self) -> Result<(), ()> {
        let (start, end) = self.site.passive;
        let listener = match bind_passive(start, end).await {
            Ok(l) => l,
            Err(e) => {
                return self
                    .reply(&format!("425 Cannot open passive port: {e}"))
                    .await
                    .map_err(|_| ())
            }
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        let ip = match self.local_ip {
            IpAddr::V4(v4) => v4,
            IpAddr::V6(_) => return self.cmd_epsv("").await, // IPv6 控制连接退回 EPSV
        };
        let o = ip.octets();
        self.pending = Some(DataMode::Passive(listener));
        self.reply(&format!(
            "227 Entering Passive Mode ({},{},{},{},{},{}).",
            o[0],
            o[1],
            o[2],
            o[3],
            port >> 8,
            port & 0xff
        ))
        .await
        .map_err(|_| ())
    }

    async fn cmd_epsv(&mut self, _arg: &str) -> Result<(), ()> {
        let (start, end) = self.site.passive;
        let listener = match bind_passive(start, end).await {
            Ok(l) => l,
            Err(e) => {
                return self
                    .reply(&format!("425 Cannot open passive port: {e}"))
                    .await
                    .map_err(|_| ())
            }
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        self.pending = Some(DataMode::Passive(listener));
        self.reply(&format!("229 Entering Extended Passive Mode (|||{port}|)"))
            .await
            .map_err(|_| ())
    }

    async fn cmd_port(&mut self, arg: &str) -> Result<(), ()> {
        let nums: Vec<u16> = arg
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if nums.len() != 6 {
            return self
                .reply("501 Invalid PORT arguments.")
                .await
                .map_err(|_| ());
        }
        let ip = IpAddr::from([nums[0] as u8, nums[1] as u8, nums[2] as u8, nums[3] as u8]);
        let port = (nums[4] << 8) | nums[5];
        self.store_active(ip, port).await
    }

    async fn cmd_eprt(&mut self, arg: &str) -> Result<(), ()> {
        let parts: Vec<&str> = arg.split('|').collect();
        if parts.len() < 4 {
            return self
                .reply("501 Invalid EPRT arguments.")
                .await
                .map_err(|_| ());
        }
        let ip: IpAddr = match parts[2].parse() {
            Ok(ip) => ip,
            Err(_) => {
                return self
                    .reply("501 Invalid EPRT address.")
                    .await
                    .map_err(|_| ())
            }
        };
        let port: u16 = match parts[3].parse() {
            Ok(p) => p,
            Err(_) => return self.reply("501 Invalid EPRT port.").await.map_err(|_| ()),
        };
        self.store_active(ip, port).await
    }

    /// 主动模式安全检查：目标必须与控制连接对端同 IP（防端口反弹扫描）
    async fn store_active(&mut self, ip: IpAddr, port: u16) -> Result<(), ()> {
        if ip != self.peer.ip() || port < 1024 {
            return self.reply("501 PORT/EPRT rejected.").await.map_err(|_| ());
        }
        self.pending = Some(DataMode::Active(SocketAddr::new(ip, port)));
        self.reply("200 Active mode set.").await.map_err(|_| ())
    }

    async fn open_data(&mut self) -> Result<TcpStream, String> {
        match self.pending.take() {
            None => Err("425 Use PORT or PASV first.".into()),
            Some(DataMode::Passive(listener)) => {
                let accept = async {
                    let (s, _) = listener.accept().await?;
                    Ok::<_, io::Error>(s)
                };
                tokio::time::timeout(DATA_WAIT, accept)
                    .await
                    .map_err(|_| "425 Passive data connection timed out.".to_string())?
                    .map_err(|e| format!("425 Data connection failed: {e}"))
            }
            Some(DataMode::Active(addr)) => {
                if addr.ip() != self.peer.ip() {
                    return Err("425 Data connection target rejected.".into());
                }
                tokio::time::timeout(CONNECT_WAIT, TcpStream::connect(addr))
                    .await
                    .map_err(|_| "425 Active data connection timed out.".to_string())?
                    .map_err(|e| format!("425 Data connection failed: {e}"))
            }
        }
    }

    // ===== 列表与传输 =====

    async fn cmd_list(&mut self, arg: &str, names_only: bool) -> Result<(), ()> {
        if !self.perms().list {
            return self
                .reply("550 权限不足：未授权浏览目录.")
                .await
                .map_err(|_| ());
        }
        let target = if arg.is_empty() { "." } else { arg };
        let vp = match resolve_fs(self.view(), &self.cur, target) {
            Ok(v) => v,
            Err(e) => return self.reply(&format!("550 {e}")).await.map_err(|_| ()),
        };
        let entries = match collect_entries(self.view(), &vp) {
            Ok(e) => e,
            Err(_) => return self.reply("550 List failed.").await.map_err(|_| ()),
        };
        let lines: Vec<String> = if names_only {
            entries.iter().map(|e| e.name.clone()).collect()
        } else {
            entries.iter().map(list_line).collect()
        };
        self.send_lines(lines).await
    }

    async fn cmd_mlsd(&mut self, arg: &str) -> Result<(), ()> {
        if !self.perms().list {
            return self
                .reply("550 权限不足：未授权浏览目录.")
                .await
                .map_err(|_| ());
        }
        let target = if arg.is_empty() { "." } else { arg };
        let Ok(vp) = resolve_fs(self.view(), &self.cur, target) else {
            return self.reply("550 非法路径.").await.map_err(|_| ());
        };
        let entries = match collect_entries(self.view(), &vp) {
            Ok(e) => e,
            Err(_) => return self.reply("550 List failed.").await.map_err(|_| ()),
        };
        let lines: Vec<String> = entries
            .iter()
            .map(|e| format!("{} {}", mlsd_facts(e), e.name))
            .collect();
        self.send_lines(lines).await
    }

    async fn cmd_mlst(&mut self, arg: &str) -> Result<(), ()> {
        if !self.perms().list {
            return self
                .reply("550 权限不足：未授权浏览目录.")
                .await
                .map_err(|_| ());
        }
        let target = if arg.is_empty() { "." } else { arg };
        let Ok(vp) = resolve_fs(self.view(), &self.cur, target) else {
            return self.reply("550 非法路径.").await.map_err(|_| ());
        };
        let real = vp.real(self.view());
        let Ok(m) = std::fs::metadata(&real) else {
            return self.reply("550 Not found.").await.map_err(|_| ());
        };
        let e = EntryInfo {
            name: vp.display(),
            is_dir: m.is_dir(),
            size: m.len(),
            mtime: m.modified().ok(),
        };
        let disp = vp.display();
        self.reply(&format!("250-Listing {disp}"))
            .await
            .map_err(|_| ())?;
        self.reply(&format!("{} {}", mlsd_facts(&e), disp))
            .await
            .map_err(|_| ())?;
        self.reply("250 End").await.map_err(|_| ())
    }

    /// 建立数据连接 → 150 → 逐行写出 → 226
    async fn send_lines(&mut self, lines: Vec<String>) -> Result<(), ()> {
        let mut data = match self.open_data().await {
            Ok(d) => d,
            Err(e) => return self.reply(&e).await.map_err(|_| ()),
        };
        if self.reply("150 Opening data connection.").await.is_err() {
            return Err(());
        }
        let mut body = lines.join("\r\n");
        if !body.is_empty() {
            body.push_str("\r\n");
        }
        let ok = data.write_all(body.as_bytes()).await.is_ok() && data.shutdown().await.is_ok();
        if ok {
            self.reply("226 Transfer complete.").await.map_err(|_| ())
        } else {
            self.reply("451 Transfer aborted.").await.map_err(|_| ())
        }
    }

    async fn cmd_retr(&mut self, arg: &str) -> Result<(), ()> {
        if !self.perms().download {
            return self
                .reply("550 权限不足：未授权下载.")
                .await
                .map_err(|_| ());
        }
        let Ok(vp) = resolve_fs(self.view(), &self.cur, arg) else {
            return self.reply("550 非法路径.").await.map_err(|_| ());
        };
        let real = vp.real(self.view());
        let Ok(md) = std::fs::metadata(&real) else {
            return self.reply("550 File not found.").await.map_err(|_| ());
        };
        if !md.is_file() {
            return self.reply("550 Not a regular file.").await.map_err(|_| ());
        }
        let mut file = match tokio::fs::File::open(&real).await {
            Ok(f) => f,
            Err(_) => return self.reply("550 Open failed.").await.map_err(|_| ()),
        };
        if self.rest_offset > 0 {
            use tokio::io::AsyncSeekExt;
            let _ = file.seek(std::io::SeekFrom::Start(self.rest_offset)).await;
        }
        self.rest_offset = 0;
        let mut data = match self.open_data().await {
            Ok(d) => d,
            Err(e) => return self.reply(&e).await.map_err(|_| ()),
        };
        if self.reply("150 Opening data connection.").await.is_err() {
            return Err(());
        }
        let ok =
            tokio::io::copy(&mut file, &mut data).await.is_ok() && data.shutdown().await.is_ok();
        if ok {
            self.reply("226 Transfer complete.").await.map_err(|_| ())
        } else {
            self.reply("451 Transfer aborted.").await.map_err(|_| ())
        }
    }

    async fn cmd_stor(&mut self, arg: &str, append: bool) -> Result<(), ()> {
        if !self.perms().upload {
            return self
                .reply("550 权限不足：未授权上传.")
                .await
                .map_err(|_| ());
        }
        let Ok(vp) = resolve_fs(self.view(), &self.cur, arg) else {
            return self.reply("550 非法路径.").await.map_err(|_| ());
        };
        if vp.is_root() || vp.at_mount_root() {
            return self.reply("550 Not a file path.").await.map_err(|_| ());
        }
        let real = vp.real(self.view());
        if std::fs::metadata(&real)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            return self
                .reply("550 Target is a directory.")
                .await
                .map_err(|_| ());
        }
        let mut file = match tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(!append)
            .append(append)
            .open(&real)
            .await
        {
            Ok(f) => f,
            Err(_) => {
                return self
                    .reply("550 Open for write failed.")
                    .await
                    .map_err(|_| ())
            }
        };
        let mut data = match self.open_data().await {
            Ok(d) => d,
            Err(e) => return self.reply(&e).await.map_err(|_| ()),
        };
        if self.reply("150 Ready to receive data.").await.is_err() {
            return Err(());
        }
        let ok = tokio::io::copy(&mut data, &mut file).await.is_ok();
        let _ = file.sync_all().await;
        if ok {
            self.reply("226 Transfer complete.").await.map_err(|_| ())
        } else {
            self.reply("451 Transfer aborted.").await.map_err(|_| ())
        }
    }
}

/// LIST/NLST 参数：跳过前导 -开头的选项标记，其余整体作为路径（支持含空格）
fn strip_list_flags(arg: &str) -> &str {
    let mut rest = arg.trim();
    while rest.starts_with('-') {
        match rest.split_once(' ') {
            Some((_, r)) => rest = r.trim_start(),
            None => return "",
        }
    }
    rest
}

/// 在被动端口区间内绑定数据监听器；0-0 = 系统分配
async fn bind_passive(start: u16, end: u16) -> io::Result<TcpListener> {
    if start == 0 && end == 0 {
        return TcpListener::bind(("0.0.0.0", 0)).await;
    }
    for p in start..=end {
        if let Ok(l) = TcpListener::bind(("0.0.0.0", p)).await {
            return Ok(l);
        }
    }
    Err(io::Error::other("被动端口区间已耗尽"))
}

/// 读取一行命令（\n 结尾，容忍 \r\n）；EOF 返回 None
async fn read_line<S: AsyncRead + Unpin>(
    stream: &mut S,
    buf: &mut Vec<u8>,
) -> io::Result<Option<String>> {
    loop {
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let mut s = String::from_utf8_lossy(&line[..line.len() - 1]).to_string();
            if s.ends_with('\r') {
                s.pop();
            }
            return Ok(Some(s));
        }
        if buf.len() > MAX_LINE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "命令行过长"));
        }
        let mut chunk = [0u8; 1024];
        let n = tokio::time::timeout(CONTROL_IDLE, stream.read(&mut chunk)).await;
        let n = match n {
            Ok(r) => r?,
            Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "空闲超时")),
        };
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}
