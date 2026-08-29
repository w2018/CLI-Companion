//! 终端调试：以服务的环境变量与工作目录打开一次性控制台
//!
//! - 环境变量 = 系统环境 + 服务定义覆盖（`%VAR%` 引用在本进程内展开），
//!   只影响新开的终端进程，绝不修改系统环境变量
//! - 机密变量会出现在该终端会话中（同用户本机，风险与手动设值一致）
//! - 终端进程独立于 GUI/daemon 生命周期，关掉窗口即结束

use crate::connection::DaemonConnection;
use cli_companion_protocol::Method;
use std::collections::HashMap;
use std::process::Command;

/// CREATE_NEW_CONSOLE：为新进程分配独立控制台窗口
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// 打开"服务环境终端"
///
/// `shell`：终端宿主，"cmd"（默认）或 "powershell"。
#[tauri::command]
pub async fn open_service_terminal(
    service_id: String,
    shell: Option<String>,
) -> Result<(), String> {
    // 1. 取服务定义（service.list 含完整定义，无单独 get 方法）
    let v = DaemonConnection::call(Method::ServiceList, None)
        .await
        .map_err(|e| e.message)?;
    let items = v
        .get("services")
        .and_then(|s| s.as_array())
        .ok_or_else(|| "service.list 响应格式无效".to_string())?;
    let def = items
        .iter()
        .find(|item| {
            item.get("service")
                .and_then(|s| s.get("id"))
                .and_then(|id| id.as_str())
                == Some(service_id.as_str())
        })
        .ok_or_else(|| format!("服务不存在: {service_id}"))?
        .get("service")
        .ok_or_else(|| "service.list 响应格式无效".to_string())?
        .clone();

    let name = def
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("服务")
        .to_string();
    let working_dir = def
        .get("working_dir")
        .map(|w| w.as_str().map(|s| s.to_string()))
        .unwrap_or(None);
    let env_vars: Vec<(String, String)> = def
        .get("env")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let name = item.get("name")?.as_str()?.to_string();
                    let value = item.get("value")?.as_str()?.to_string();
                    Some((name, value))
                })
                .collect()
        })
        .unwrap_or_default();

    spawn_terminal(shell.as_deref(), &name, working_dir.as_deref(), &env_vars)
}

/// 组装并拉起终端进程（独立函数便于测试参数组装）
fn spawn_terminal(
    shell: Option<&str>,
    service_name: &str,
    working_dir: Option<&str>,
    env_vars: &[(String, String)],
) -> Result<(), String> {
    let title = format!("{service_name} · CLI Companion 调试终端");
    let (program, args) = shell_command(shell, &title);

    let mut cmd = Command::new(&program);
    cmd.args(&args);
    for (k, v) in build_env_overrides(env_vars) {
        cmd.env(k, v);
    }
    // 工作目录：展开后必须真实存在才设置，否则交给系统默认
    if let Some(wd) = working_dir.map(expand_percent) {
        if !wd.is_empty() && std::path::Path::new(&wd).is_dir() {
            cmd.current_dir(wd);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NEW_CONSOLE);
    }
    cmd.spawn().map_err(|e| format!("打开终端失败: {e}"))?;
    Ok(())
}

/// 终端宿主 → (程序, 参数)；标题用于窗口识别
fn shell_command(shell: Option<&str>, title: &str) -> (String, Vec<String>) {
    match shell {
        Some("powershell") => (
            "powershell.exe".into(),
            vec![
                "-NoLogo".into(),
                "-NoExit".into(),
                "-Command".into(),
                format!("$Host.UI.RawUI.WindowTitle='{}'", title.replace('\'', "''")),
            ],
        ),
        // 默认 cmd
        _ => (
            "cmd.exe".into(),
            vec!["/K".into(), format!("title {title}")],
        ),
    }
}

/// 服务环境变量 → 进程环境覆盖（展开 `%VAR%` 引用；变量名为空则跳过）
fn build_env_overrides(env_vars: &[(String, String)]) -> Vec<(String, String)> {
    env_vars
        .iter()
        .filter(|(name, _)| !name.is_empty())
        .map(|(name, value)| (name.clone(), expand_percent(value)))
        .collect()
}

/// 展开 `%VAR%` 形式的环境变量引用（可出现多次；未定义的保留原样）
fn expand_percent(input: &str) -> String {
    let system: HashMap<String, String> = std::env::vars().collect();
    expand_percent_with(input, &system)
}

fn expand_percent_with(input: &str, env: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            // 寻找配对的结束 %（仅 ASCII 名称；含中文的段不视为变量）
            if let Some(rel) = input[i + 1..].find('%') {
                let name = &input[i + 1..i + 1 + rel];
                if !name.is_empty() && name.is_ascii() {
                    match env.get(name) {
                        Some(v) => {
                            out.push_str(v);
                            i = i + 1 + rel + 1;
                            continue;
                        }
                        None => {
                            // 未定义：保留原样（含两个 %）
                            out.push('%');
                            i += 1;
                            continue;
                        }
                    }
                }
            }
        }
        // UTF-8 安全推进：非 ASCII 按字符边界复制
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&input[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn cmd默认与powershell参数() {
        let (p, a) = shell_command(None, "web · 调试终端");
        assert_eq!(p, "cmd.exe");
        assert_eq!(
            a,
            vec!["/K".to_string(), "title web · 调试终端".to_string()]
        );

        let (p, a) = shell_command(Some("powershell"), "it's web");
        assert_eq!(p, "powershell.exe");
        // 单引号被成对转义
        assert!(a[3].contains("it''s web"));
    }

    #[test]
    fn 环境变量值展开引用() {
        let e = env(&[("JAVA_HOME", "C:\\jdk"), ("PORT", "8080")]);
        assert_eq!(expand_percent_with("%JAVA_HOME%\\bin", &e), "C:\\jdk\\bin");
        assert_eq!(expand_percent_with("--port=%PORT%", &e), "--port=8080");
        // 多次引用
        assert_eq!(expand_percent_with("%PORT%:%PORT%", &e), "8080:8080");
    }

    #[test]
    fn 未定义变量保留原样() {
        let e = env(&[]);
        assert_eq!(expand_percent_with("%NOT_SET%X", &e), "%NOT_SET%X");
        // 单个 % 原样保留
        assert_eq!(expand_percent_with("100% sure", &e), "100% sure");
    }

    #[test]
    fn 中文与空名不误判() {
        let e = env(&[("HOME", "/h")]);
        assert_eq!(expand_percent_with("%中文%", &e), "%中文%");
        assert_eq!(expand_percent_with("%%", &e), "%%");
        assert_eq!(expand_percent_with("值%HOME%尾", &e), "值/h尾");
    }

    #[test]
    fn 覆盖集跳过空名并展开() {
        let vars = [
            ("".to_string(), "被跳过".to_string()),
            ("A".to_string(), "%B%".to_string()),
        ];
        let e = env(&[("B", "2")]);
        // build_env_overrides 用真实系统环境展开，这里只验证过滤与映射逻辑
        let out: Vec<(String, String)> = vars
            .iter()
            .filter(|(n, _)| !n.is_empty())
            .map(|(n, v)| (n.clone(), expand_percent_with(v, &e)))
            .collect();
        assert_eq!(out, vec![("A".to_string(), "2".to_string())]);
    }
}
