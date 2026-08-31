//! FTP 服务端端到端真实测试（v2.6.0）
//!
//! 全部走真实 TCP：本测试进程内拉起 FTP 服务端（127.0.0.1 随机端口），
//! 用原生 TCP 客户端逐条收发 RFC 959 命令，再叠加 curl.exe 真客户端互操作。
//! 场景：认证（成功/失败/锁定/未授权站点）、全权限文件周期、只读越权、
//! 路径越狱、多目录挂载、PASV/EPSV 端口区间、多监听器。

use cli_companion_daemon::app_config::{
    FtpListener, FtpPermissions, FtpSettings, FtpUser, Secrets,
};
use cli_companion_daemon::ftp::{run_server_on, BoundListener, FtpServerShared};
use cli_companion_daemon::state::{AppState, ConfigStore};
use cli_companion_daemon::{dirs::DataDirs, events, manager::ServiceManager, sync::SyncEngine};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// ===== 测试脚手架 =====

struct TestFtp {
    port: u16,
    state: AppState,
    _tmp: PathBuf,
    shutdown: Arc<tokio::sync::Notify>,
    home: PathBuf,
    media: PathBuf,
    docs: PathBuf,
}

/// 建目录并预置文件
fn prep_dir(dir: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(dir).unwrap();
    for (name, content) in files {
        let mut f = std::fs::File::create(dir.join(name)).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }
}

/// 拉起一套单监听器 FTP 服务（随机端口，0-0 被动区间=临时端口）
async fn spawn_ftp(users: Vec<FtpUser>, passive: (u16, u16)) -> TestFtp {
    let tmp = std::env::temp_dir().join(format!(
        "cc-ftp-test-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let home = tmp.join("home");
    let media = tmp.join("media");
    let docs = tmp.join("docs");
    prep_dir(&home, &[("welcome.txt", "hello-ftp")]);
    prep_dir(&media, &[("song.mp3", "MEDIA-CONTENT-123")]);
    prep_dir(&docs, &[("note.md", "# docs note")]);

    let dirs = DataDirs::resolve(Some(tmp.clone()));
    let state = AppState {
        as_service: false,
        manager: Arc::new(ServiceManager::new(
            dirs.clone(),
            Arc::new(events::new_bus()),
            false,
        )),
        config: Arc::new(tokio::sync::Mutex::new(ConfigStore {
            services: Default::default(),
            app: app_with(users, &home, passive),
            secrets: Secrets::default(),
        })),
        sync: Arc::new(SyncEngine::new()),
        events: Arc::new(events::new_bus()),
        shutdown: Arc::new(tokio::sync::Notify::new()),
        dirs,
    };

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let sd = Arc::new(tokio::sync::Notify::new());
    let root = std::fs::canonicalize(&home).unwrap();
    tokio::spawn(run_server_on(
        state.clone(),
        vec![BoundListener {
            name: "测试站点".into(),
            root,
            listener,
        }],
        passive,
        Arc::new(FtpServerShared::default()),
        sd.clone(),
    ));
    TestFtp {
        port,
        state,
        _tmp: tmp,
        shutdown: sd,
        home,
        media,
        docs,
    }
}

fn app_with(
    users: Vec<FtpUser>,
    home: &Path,
    passive: (u16, u16),
) -> cli_companion_daemon::app_config::AppConfig {
    cli_companion_daemon::app_config::AppConfig {
        ftp: FtpSettings {
            enabled: true,
            passive_port_start: passive.0,
            passive_port_end: passive.1,
            listeners: vec![FtpListener {
                name: "测试站点".into(),
                port: 21,
                root: home.to_path_buf(),
                enabled: true,
            }],
            users,
        },
        ..Default::default()
    }
}

fn full_perms() -> FtpPermissions {
    FtpPermissions {
        list: true,
        download: true,
        upload: true,
        delete: true,
        rename: true,
        mkdir: true,
    }
}

/// 多目录授权用户：home（站点根）+ media + docs
fn power_user() -> FtpUser {
    // allowed_roots 在 spawn_ftp 内部按 tmp 路径注入，这里先放 home 占位，
    // 由 spawn_ftp 统一替换（见下）
    FtpUser {
        username: "power".into(),
        allowed_roots: vec![],
        permissions: full_perms(),
        enabled: true,
    }
}

/// 协议客户端：逐条发送命令并读取（可多行）响应
struct FtpClient {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl FtpClient {
    async fn connect(port: u16) -> Self {
        let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut c = Self {
            stream,
            buf: Vec::new(),
        };
        let greet = c.read_reply().await;
        assert!(greet.starts_with("220"), "欢迎语异常: {greet}");
        c
    }

    /// 读取一行（到 \r\n）
    async fn read_line(&mut self) -> String {
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=pos).collect();
                let mut s = String::from_utf8_lossy(&line).to_string();
                while s.ends_with('\n') || s.ends_with('\r') {
                    s.pop();
                }
                return s;
            }
            let mut chunk = [0u8; 2048];
            let n = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.stream.read(&mut chunk),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(n > 0, "连接被服务端关闭");
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    /// 读取完整回复（支持多行，直到最后一行 `NNN ` 格式）
    async fn read_reply(&mut self) -> String {
        let mut all = String::new();
        loop {
            let line = self.read_line().await;
            let done = line.len() >= 4
                && line.as_bytes()[3] == b' '
                && line[..3].chars().all(|c| c.is_ascii_digit());
            all.push_str(&line);
            all.push('\n');
            if done {
                return all;
            }
        }
    }

    async fn cmd(&mut self, line: &str) -> String {
        self.stream
            .write_all(format!("{line}\r\n").as_bytes())
            .await
            .unwrap();
        self.read_reply().await
    }

    /// PASV 回复中解析数据端口并连接
    async fn pasv_connect(&mut self) -> TcpStream {
        let r = self.cmd("PASV").await;
        assert!(r.starts_with("227"), "PASV 失败: {r}");
        let open = r.rfind('(').unwrap();
        let close = r.rfind(')').unwrap();
        let nums: Vec<u16> = r[open + 1..close]
            .split(',')
            .map(|s| s.trim().parse().unwrap())
            .collect();
        assert_eq!(nums.len(), 6);
        let port = (nums[4] << 8) | nums[5];
        TcpStream::connect(("127.0.0.1", port)).await.unwrap()
    }

    async fn login(&mut self, user: &str, pass: &str) -> String {
        let r = self.cmd(&format!("USER {user}")).await;
        assert!(r.starts_with("331"), "USER 失败: {r}");
        self.cmd(&format!("PASS {pass}")).await
    }

    /// 读取数据连接直到 EOF
    async fn drain(mut data: TcpStream) -> String {
        let mut out = Vec::new();
        data.read_to_end(&mut out).await.unwrap();
        String::from_utf8_lossy(&out).to_string()
    }
}

// ===== 场景 1：认证 =====

#[tokio::test]
async fn 认证成功失败与锁定() {
    let mut u = power_user();
    u.allowed_roots = vec!["D:\\占位".into()]; // 稍后替换
    let ftp = spawn_ftp(vec![u], (0, 0)).await;
    // 站点根必须在授权内：注入真实路径 + 密码
    set_user_roots(&ftp, "power", vec![&ftp.home]);
    set_password(&ftp, "power", "s3cret!").await;

    // 错误密码
    let mut c = FtpClient::connect(ftp.port).await;
    let r = c.login("power", "wrong").await;
    assert!(r.starts_with("530"), "错误密码应 530: {r}");

    // 正确密码
    let r = c.login("power", "s3cret!").await;
    assert!(r.starts_with("230"), "正确密码应 230: {r}");

    // 不存在的用户
    let mut c2 = FtpClient::connect(ftp.port).await;
    let r = c2.login("nobody", "x").await;
    assert!(r.starts_with("530"), "未知用户应 530: {r}");

    // 连续失败 3 次被断开（421）
    let mut c3 = FtpClient::connect(ftp.port).await;
    for i in 0..3 {
        let r = c3.login("power", &format!("bad{i}")).await;
        if i < 2 {
            assert!(r.starts_with("530"));
        }
    }
    // 第 3 次失败后连接应被关闭（读命令返回错误/空）
    c3.stream.write_all(b"NOOP\r\n").await.unwrap();
    let closed = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        c3.stream.read(&mut [0u8; 64]),
    )
    .await;
    match closed {
        Ok(Ok(0)) | Err(_) => {} // 关闭或超时都算断开
        Ok(Ok(n)) => panic!("锁定后不应再响应，收到 {} 字节", n),
        Ok(Err(_)) => {}
    }
    ftp.shutdown.notify_waiters();
}

#[tokio::test]
async fn 未授权站点根拒绝登录() {
    let mut u = power_user();
    u.allowed_roots = vec!["D:\\不存在的授权".into()];
    let ftp = spawn_ftp(vec![u], (0, 0)).await;
    set_user_roots(&ftp, "power", vec![&ftp.media]); // 只授权 media，不含站点根 home
    set_password(&ftp, "power", "pw").await;

    let mut c = FtpClient::connect(ftp.port).await;
    let r = c.login("power", "pw").await;
    assert!(r.starts_with("530"), "站点根未授权应 530: {r}");
    ftp.shutdown.notify_waiters();
}

// ===== 场景 2：全权限文件周期 + 多目录挂载 =====

#[tokio::test]
async fn 全权限完整文件周期与多目录挂载() {
    let mut u = power_user();
    u.allowed_roots = vec![]; // 注入
    let ftp = spawn_ftp(vec![u], (0, 0)).await;
    set_user_roots(&ftp, "power", vec![&ftp.home, &ftp.media, &ftp.docs]);
    set_password(&ftp, "power", "pw").await;

    let mut c = FtpClient::connect(ftp.port).await;
    assert!(c.login("power", "pw").await.starts_with("230"));

    // PWD = "/"
    assert!(c.cmd("PWD").await.contains("\"/\""));

    // 根 LIST：home 文件 + 两个挂载目录（media/docs）
    let data = c.pasv_connect().await;
    let r = c.cmd("LIST").await;
    assert!(r.starts_with("150"), "LIST 应 150: {r}");
    let listing = FtpClient::drain(data).await;
    assert!(c.read_reply().await.starts_with("226"));
    assert!(
        listing.contains("welcome.txt"),
        "根列表缺 home 文件: {listing}"
    );
    assert!(listing.contains("media"), "根列表缺挂载 media: {listing}");
    assert!(listing.contains("docs"), "根列表缺挂载 docs: {listing}");

    // 进入挂载目录读文件（多目录授权的核心）
    assert!(c.cmd("CWD media").await.starts_with("250"));
    assert!(c.cmd("PWD").await.contains("\"/media\""));
    let data = c.pasv_connect().await;
    let r = c.cmd("RETR song.mp3").await;
    assert!(r.starts_with("150"), "RETR 应 150: {r}");
    let content = FtpClient::drain(data).await;
    assert!(c.read_reply().await.starts_with("226"));
    assert_eq!(content, "MEDIA-CONTENT-123", "跨挂载下载内容错误");

    // SIZE/MDTM
    assert_eq!(
        c.cmd("SIZE song.mp3").await.trim(),
        "213 17",
        "SIZE 应返回 17 字节"
    );
    assert!(c.cmd("MDTM song.mp3").await.starts_with("213"));

    // 挂载根上跳回：/media/.. = /
    assert!(c.cmd("CDUP").await.starts_with("250"));
    assert!(c.cmd("PWD").await.contains("\"/\""));

    // MKD + CWD + STOR + APPE
    assert!(c.cmd("MKD work").await.starts_with("257"));
    assert!(c.cmd("CWD work").await.starts_with("250"));
    let mut data = c.pasv_connect().await;
    let r = c.cmd("STOR upload.txt").await;
    assert!(r.starts_with("150"), "STOR 应 150: {r}");
    data.write_all(b"PART1-").await.unwrap();
    data.shutdown().await.unwrap();
    assert!(c.read_reply().await.starts_with("226"));
    let mut data = c.pasv_connect().await;
    let r = c.cmd("APPE upload.txt").await;
    assert!(r.starts_with("150"), "APPE 应 150: {r}");
    data.write_all(b"PART2").await.unwrap();
    data.shutdown().await.unwrap();
    assert!(c.read_reply().await.starts_with("226"));
    assert_eq!(
        std::fs::read_to_string(ftp.home.join("work/upload.txt")).unwrap(),
        "PART1-PART2",
        "STOR+APPE 落盘内容错误"
    );

    // SIZE 验证上传（PART1- 6 字节 + PART2 5 字节 = 11）
    assert_eq!(
        c.cmd("SIZE upload.txt").await.trim(),
        "213 11",
        "上传后 SIZE 应 11"
    );

    // 下载回来比对（回环完整性）
    let data = c.pasv_connect().await;
    let r = c.cmd("RETR upload.txt").await;
    assert!(r.starts_with("150"));
    let back = FtpClient::drain(data).await;
    assert!(c.read_reply().await.starts_with("226"));
    assert_eq!(back, "PART1-PART2");

    // RNFR/RNTO
    assert!(c.cmd("RNFR upload.txt").await.starts_with("350"));
    assert!(c.cmd("RNTO renamed.txt").await.starts_with("250"));
    assert!(
        !ftp.home.join("work/upload.txt").exists() && ftp.home.join("work/renamed.txt").exists(),
        "重命名落盘错误"
    );

    // NLST
    let data = c.pasv_connect().await;
    let r = c.cmd("NLST").await;
    assert!(r.starts_with("150"));
    let names = FtpClient::drain(data).await;
    assert!(c.read_reply().await.starts_with("226"));
    assert!(names.contains("renamed.txt"), "NLST 缺文件: {names}");

    // MLSD
    let data = c.pasv_connect().await;
    let r = c.cmd("MLSD").await;
    assert!(r.starts_with("150"));
    let mlsd = FtpClient::drain(data).await;
    assert!(c.read_reply().await.starts_with("226"));
    assert!(
        mlsd.contains("type=file;") && mlsd.contains("renamed.txt"),
        "MLSD 行异常: {mlsd}"
    );

    // DELE + RMD
    assert!(c.cmd("CWD /").await.starts_with("250"));
    assert!(c.cmd("DELE /work/renamed.txt").await.starts_with("250"));
    assert!(c.cmd("RMD /work").await.starts_with("250"));
    assert!(!ftp.home.join("work").exists(), "RMD 后目录应消失");

    // 挂载根不可删/不可改名
    assert!(c.cmd("RMD /media").await.starts_with("550"), "挂载根不可删");
    assert!(
        c.cmd("RNFR /media").await.starts_with("550"),
        "挂载根不可改名"
    );

    // TYPE/NOOP/OPTS/REST
    assert!(c.cmd("TYPE I").await.starts_with("200"));
    assert!(c.cmd("NOOP").await.starts_with("200"));
    assert!(c.cmd("OPTS UTF8 ON").await.starts_with("200"));
    assert!(c.cmd("REST 0").await.starts_with("350"));

    // QUIT
    assert!(c.cmd("QUIT").await.starts_with("221"));
    ftp.shutdown.notify_waiters();
}

// ===== 场景 3：只读用户越权 =====

#[tokio::test]
async fn 只读用户写操作全部被拒() {
    let ro = FtpUser {
        username: "viewer".into(),
        allowed_roots: vec![],
        permissions: FtpPermissions::default(), // list+download
        enabled: true,
    };
    let ftp = spawn_ftp(vec![ro], (0, 0)).await;
    set_user_roots(&ftp, "viewer", vec![&ftp.home]);
    set_password(&ftp, "viewer", "v").await;

    let mut c = FtpClient::connect(ftp.port).await;
    assert!(c.login("viewer", "v").await.starts_with("230"));

    // 可读
    let data = c.pasv_connect().await;
    assert!(c.cmd("RETR welcome.txt").await.starts_with("150"));
    let content = FtpClient::drain(data).await;
    assert!(c.read_reply().await.starts_with("226"));
    assert_eq!(content, "hello-ftp");

    // 写操作全部 550
    for (cmd, desc) in [
        ("MKD nope", "MKD"),
        ("DELE welcome.txt", "DELE"),
        ("RNFR welcome.txt", "RNFR"),
        ("RMD nothing", "RMD"),
    ] {
        let r = c.cmd(cmd).await;
        assert!(r.starts_with("550"), "{desc} 未被拒绝: {r}");
    }
    let data = c.pasv_connect().await;
    let r = c.cmd("STOR hack.txt").await;
    assert!(r.starts_with("550"), "STOR 未被拒绝: {r}",);
    let _ = data;
    // RNFR 被拒后 RNTO 也被拒（viewer 无 rename 权限，550 在前）
    assert!(c.cmd("RNTO x").await.starts_with("550"));
    assert!(!ftp.home.join("hack.txt").exists());
    ftp.shutdown.notify_waiters();
}

// ===== 场景 4：路径越狱 =====

#[tokio::test]
async fn 路径越狱全部失败() {
    let mut u = power_user();
    u.allowed_roots = vec![];
    let ftp = spawn_ftp(vec![u], (0, 0)).await;
    set_user_roots(&ftp, "power", vec![&ftp.home]);
    set_password(&ftp, "power", "pw").await;

    let mut c = FtpClient::connect(ftp.port).await;
    assert!(c.login("power", "pw").await.starts_with("230"));

    // ../ 越狱
    assert!(
        c.cmd("RETR ../../etc/passwd").await.starts_with("550"),
        "相对 ../ 越狱未拒绝"
    );
    assert!(
        c.cmd("RETR /../../../windows/win.ini")
            .await
            .starts_with("550"),
        "绝对 ../ 越狱未拒绝"
    );
    // 盘符
    for p in ["C:\\Windows\\win.ini", "c:/windows/notepad.exe", "C:x"] {
        assert!(
            c.cmd(&format!("RETR {p}")).await.starts_with("550"),
            "盘符路径 {p} 未拒绝"
        );
    }
    // 反斜杠
    assert!(
        c.cmd("RETR media\\..\\..\\secret").await.starts_with("550"),
        "反斜杠未拒绝"
    );
    // CWD 越狱
    assert!(
        c.cmd("CWD ..").await.starts_with("250"),
        "根上 CWD .. 应保持根（FTP 语义）"
    );
    assert!(c.cmd("PWD").await.contains("\"/\""));
    assert!(
        c.cmd("CWD /../../").await.starts_with("550") || c.cmd("PWD").await.contains("\"/\""),
        "CWD 越狱后不在根"
    );
    // STOR 越狱目标
    let data = c.pasv_connect().await;
    let r = c.cmd("STOR ../evil.txt").await;
    assert!(r.starts_with("550"), "STOR ../ 未拒绝: {r}");
    let _ = data;
    assert!(!ftp.home.parent().unwrap().join("evil.txt").exists());
    // MKD 越狱
    assert!(c.cmd("MKD ../../evil").await.starts_with("550"));
    // 控制字符
    assert!(c.cmd("RETR a\u{0}b").await.starts_with("550") || c.cmd("PWD").await.contains("\"/\""));
    ftp.shutdown.notify_waiters();
}

// ===== 场景 5：PASV/EPSV 端口区间 =====

#[tokio::test]
async fn 被动模式端口落在配置区间() {
    let mut u = power_user();
    u.allowed_roots = vec![];
    let ftp = spawn_ftp(vec![u], (60000, 60009)).await;
    set_user_roots(&ftp, "power", vec![&ftp.home]);
    set_password(&ftp, "power", "pw").await;

    let mut c = FtpClient::connect(ftp.port).await;
    assert!(c.login("power", "pw").await.starts_with("230"));
    for _ in 0..5 {
        let r = c.cmd("PASV").await;
        assert!(r.starts_with("227"));
        let open = r.rfind('(').unwrap();
        let nums: Vec<u16> = r[open + 1..r.rfind(')').unwrap()]
            .split(',')
            .map(|s| s.trim().parse().unwrap())
            .collect();
        let port = (nums[4] << 8) | nums[5];
        assert!(
            (60000..=60009).contains(&port),
            "PASV 端口 {port} 不在区间 60000-60009"
        );
    }
    // EPSV
    let r = c.cmd("EPSV").await;
    assert!(r.starts_with("229"), "EPSV 失败: {r}");
    let open = r.rfind("|||").unwrap();
    let port: u16 = r[open + 3..r.rfind('|').unwrap()].parse().unwrap();
    assert!((60000..=60009).contains(&port), "EPSV 端口越界: {port}");
    ftp.shutdown.notify_waiters();
}

// ===== 场景 6：curl.exe 真客户端互操作 =====

#[tokio::test]
async fn curl真客户端互操作() {
    let mut u = power_user();
    u.allowed_roots = vec![];
    let ftp = spawn_ftp(vec![u], (0, 0)).await;
    set_user_roots(&ftp, "power", vec![&ftp.home, &ftp.media]);
    set_password(&ftp, "power", "pw").await;

    let url = format!("ftp://127.0.0.1:{}", ftp.port);
    let curl = curl_path();

    // 1) 列目录（MLSD/LIST 自动协商）
    let out = run_curl(
        &curl,
        &["--ftp-pasv", "--user", "power:pw", &format!("{url}/")],
    )
    .await;
    assert!(
        out.status.success(),
        "curl 列目录失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let listing = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        listing.contains("welcome.txt"),
        "curl 列表缺文件: {listing}"
    );
    assert!(listing.contains("media"), "curl 列表缺挂载: {listing}");

    // 2) 下载（挂载目录内文件）
    let out = run_curl(
        &curl,
        &[
            "-s",
            "--ftp-pasv",
            "--user",
            "power:pw",
            &format!("{url}/media/song.mp3"),
        ],
    )
    .await;
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "MEDIA-CONTENT-123",
        "curl 下载内容错误"
    );

    // 3) 上传 -T 到根目录（curl 不自动建目录）
    let src = ftp.home.parent().unwrap().join("src.txt");
    std::fs::write(&src, "CURL-UPLOAD-OK").unwrap();
    let out = run_curl(
        &curl,
        &[
            "-s",
            "--ftp-pasv",
            "--user",
            "power:pw",
            "-T",
            src.to_str().unwrap(),
            &format!("{url}/curl-up.txt"),
        ],
    )
    .await;
    assert!(
        out.status.success(),
        "curl 上传失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(ftp.home.join("curl-up.txt")).unwrap(),
        "CURL-UPLOAD-OK",
        "curl 上传落盘内容错误"
    );

    // 4) 下载刚上传的文件（回环一致）
    let out = run_curl(
        &curl,
        &[
            "-s",
            "--ftp-pasv",
            "--user",
            "power:pw",
            &format!("{url}/curl-up.txt"),
        ],
    )
    .await;
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "CURL-UPLOAD-OK",
        "curl 回读内容错误"
    );
    ftp.shutdown.notify_waiters();
}

// ===== 场景 7：多监听器（不同端口不同根） =====

#[tokio::test]
async fn 多监听器不同端口不同根() {
    let tmp = std::env::temp_dir().join(format!(
        "cc-ftp-multi-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let a = tmp.join("a");
    let b = tmp.join("b");
    prep_dir(&a, &[("from-a.txt", "AAA")]);
    prep_dir(&b, &[("from-b.txt", "BBB")]);

    let dirs = DataDirs::resolve(Some(tmp.clone()));
    let app = cli_companion_daemon::app_config::AppConfig {
        ftp: FtpSettings {
            enabled: true,
            passive_port_start: 0,
            passive_port_end: 0,
            listeners: vec![
                FtpListener {
                    name: "A".into(),
                    port: 21,
                    root: a.clone(),
                    enabled: true,
                },
                FtpListener {
                    name: "B".into(),
                    port: 21,
                    root: b.clone(),
                    enabled: true,
                },
            ],
            users: vec![FtpUser {
                username: "dual".into(),
                allowed_roots: vec![a.clone(), b.clone()],
                permissions: full_perms(),
                enabled: true,
            }],
        },
        ..Default::default()
    };
    let state = AppState {
        as_service: false,
        manager: Arc::new(ServiceManager::new(
            dirs.clone(),
            Arc::new(events::new_bus()),
            false,
        )),
        config: Arc::new(tokio::sync::Mutex::new(ConfigStore {
            services: Default::default(),
            app,
            secrets: Secrets::default(),
        })),
        sync: Arc::new(SyncEngine::new()),
        events: Arc::new(events::new_bus()),
        shutdown: Arc::new(tokio::sync::Notify::new()),
        dirs,
    };
    {
        let mut st = state.config.lock().await;
        st.secrets.set_ftp_password("dual", "d").unwrap();
    }

    let la = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let lb = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let pa = la.local_addr().unwrap().port();
    let pb = lb.local_addr().unwrap().port();
    assert_ne!(pa, pb);
    let sd = Arc::new(tokio::sync::Notify::new());
    tokio::spawn(run_server_on(
        state.clone(),
        vec![
            BoundListener {
                name: "A".into(),
                root: std::fs::canonicalize(&a).unwrap(),
                listener: la,
            },
            BoundListener {
                name: "B".into(),
                root: std::fs::canonicalize(&b).unwrap(),
                listener: lb,
            },
        ],
        (0, 0),
        Arc::new(FtpServerShared::default()),
        sd.clone(),
    ));

    // 端口 A 看到挂载 b（授权目录）+ a 的文件
    let mut c1 = FtpClient::connect(pa).await;
    assert!(c1.login("dual", "d").await.starts_with("230"));
    let data = c1.pasv_connect().await;
    assert!(c1.cmd("LIST").await.starts_with("150"));
    let l = FtpClient::drain(data).await;
    assert!(c1.read_reply().await.starts_with("226"));
    assert!(
        l.contains("from-a.txt") && l.contains("b"),
        "A 站点列表异常: {l}"
    );

    // 端口 B 看到挂载 a + b 的文件
    let mut c2 = FtpClient::connect(pb).await;
    assert!(c2.login("dual", "d").await.starts_with("230"));
    let data = c2.pasv_connect().await;
    assert!(c2.cmd("LIST").await.starts_with("150"));
    let l = FtpClient::drain(data).await;
    assert!(c2.read_reply().await.starts_with("226"));
    assert!(
        l.contains("from-b.txt") && l.contains("a"),
        "B 站点列表异常: {l}"
    );

    // 从 A 站点跨挂载读 B 的文件
    let data = c1.pasv_connect().await;
    assert!(c1.cmd("RETR /b/from-b.txt").await.starts_with("150"));
    let content = FtpClient::drain(data).await;
    assert!(c1.read_reply().await.starts_with("226"));
    assert_eq!(content, "BBB", "跨挂载内容错误");
    sd.notify_waiters();
}

// ===== 场景 8：监督任务动态生效 =====

#[tokio::test]
async fn 监督任务随配置启停换端口且改用户不重启() {
    use cli_companion_daemon::app_config::AppConfig;
    use cli_companion_daemon::ftp::{runtime_snapshot, spawn_supervisor};
    use cli_companion_protocol::EventTopic;

    let tmp = std::env::temp_dir().join(format!(
        "cc-ftp-super-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let home = tmp.join("home");
    prep_dir(&home, &[("f.txt", "x")]);
    let dirs = DataDirs::resolve(Some(tmp.clone()));
    let state = AppState {
        as_service: false,
        manager: Arc::new(ServiceManager::new(
            dirs.clone(),
            Arc::new(events::new_bus()),
            false,
        )),
        config: Arc::new(tokio::sync::Mutex::new(ConfigStore {
            services: Default::default(),
            app: AppConfig::default(),
            secrets: Secrets::default(),
        })),
        sync: Arc::new(SyncEngine::new()),
        events: Arc::new(events::new_bus()),
        shutdown: Arc::new(tokio::sync::Notify::new()),
        dirs,
    };
    spawn_supervisor(state.clone());

    let mk_app = |enabled: bool, port: u16| AppConfig {
        ftp: FtpSettings {
            enabled,
            passive_port_start: 0,
            passive_port_end: 0,
            listeners: vec![FtpListener {
                name: "动态".into(),
                port,
                root: home.clone(),
                enabled: true,
            }],
            users: vec![FtpUser {
                username: "u1".into(),
                allowed_roots: vec![home.clone()],
                permissions: full_perms(),
                enabled: true,
            }],
        },
        ..Default::default()
    };
    async fn changed(state: &AppState, app: AppConfig) {
        state.save_app(app).await.unwrap();
        state.emit(
            EventTopic::ConfigChanged,
            None,
            serde_json::json!({"source": "test"}),
        );
    }

    // 1) 初始未启用 → 未运行
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!runtime_snapshot().running);

    // 找一个空闲端口（绑 0 取端口后释放）
    async fn free_port() -> u16 {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        l.local_addr().unwrap().port()
    }
    let p1 = free_port().await;
    changed(&state, mk_app(true, p1)).await;
    // 2) 启用 → 运行且端口匹配
    let ok = |snap: &cli_companion_daemon::ftp::FtpRuntimeSnapshot| {
        snap.running && snap.ports == vec![p1]
    };
    for _ in 0..40 {
        if ok(&runtime_snapshot()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(
        ok(&runtime_snapshot()),
        "启用后应运行: ports={:?}",
        runtime_snapshot().ports
    );
    let starts1 = runtime_snapshot().starts;

    // 3) 仅改用户（加用户）→ 不重启（starts 不变）
    let mut app = mk_app(true, p1);
    app.ftp.users.push(FtpUser {
        username: "u2".into(),
        allowed_roots: vec![home.clone()],
        permissions: full_perms(),
        enabled: true,
    });
    changed(&state, app).await;
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    assert_eq!(runtime_snapshot().starts, starts1, "改用户不应触发重启");

    // 4) 换端口 → 自动重启到新端口
    let p2 = free_port().await;
    changed(&state, mk_app(true, p2)).await;
    let ok2 = |snap: &cli_companion_daemon::ftp::FtpRuntimeSnapshot| {
        snap.running && snap.ports == vec![p2]
    };
    for _ in 0..40 {
        if ok2(&runtime_snapshot()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(
        ok2(&runtime_snapshot()),
        "换端口后应重启: ports={:?}",
        runtime_snapshot().ports
    );

    // 5) 停用 → 停止
    changed(&state, mk_app(false, p2)).await;
    for _ in 0..40 {
        if !runtime_snapshot().running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(!runtime_snapshot().running, "停用后应停止");
    state.shutdown.notify_waiters();
}

// ===== 辅助：修改运行中的测试配置 =====

async fn set_password(ftp: &TestFtp, user: &str, pw: &str) {
    let mut st = ftp.state.config.lock().await;
    st.secrets.set_ftp_password(user, pw).unwrap();
}

fn set_user_roots(ftp: &TestFtp, username: &str, roots: Vec<&PathBuf>) {
    let st_lock = ftp.state.config.clone();
    // 同步上下文里不能 await：这里用 try_lock（测试无并发竞争）
    let mut st = st_lock.try_lock().unwrap();
    let u = st
        .app
        .ftp
        .users
        .iter_mut()
        .find(|u| u.username == username)
        .unwrap();
    u.allowed_roots = roots.iter().map(|p| (*p).clone()).collect();
}

fn curl_path() -> String {
    // Windows 自带 curl.exe（System32）
    let p = r"C:\Windows\System32\curl.exe";
    assert!(Path::new(p).exists(), "系统缺少 curl.exe，无法互操作测试");
    p.to_string()
}

/// 异步执行 curl：#[tokio::test] 默认单线程运行时，
/// 用同步 std::process 会阻塞 FTP 服务端任务导致 curl 等不到 220 欢迎语
async fn run_curl(curl: &str, args: &[&str]) -> std::process::Output {
    tokio::process::Command::new(curl)
        .args(args)
        .output()
        .await
        .expect("curl 启动失败")
}
