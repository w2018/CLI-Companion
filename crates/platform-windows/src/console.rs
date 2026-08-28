//! 控制台创建标志映射（开发文档 §4.3）
//!
//! 将 ConsoleMode / WindowStartup 映射为 CreateProcess 标志与 STARTUPINFO 值。

use cli_companion_domain::{ConsoleConfig, ConsoleMode, WindowStartup};

/// CreateProcess 派生进程标志
#[derive(Debug, Clone, Copy)]
pub struct ConsoleFlags {
    /// 传给 CommandExt::creation_flags 的部分
    pub creation_flags: u32,
    /// 传给 STARTUPINFO wShowWindow 的值（SW_*）
    pub show_window: i16,
}

// STARTUPINFO wShowWindow 常量
const SW_HIDE: i16 = 0;
const SW_SHOWNORMAL: i16 = 1;
const SW_SHOWMINNOACTIVE: i16 = 7;

// CreateProcess 标志
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;

/// 将控制台配置映射为进程创建标志
pub fn creation_flags(console: &ConsoleConfig) -> ConsoleFlags {
    let (flag, show) = match (console.mode, console.startup) {
        (ConsoleMode::NewConsoleVisible, WindowStartup::Normal) => (CREATE_NEW_CONSOLE, SW_SHOWNORMAL),
        (ConsoleMode::NewConsoleVisible, WindowStartup::Minimized) => (CREATE_NEW_CONSOLE, SW_SHOWMINNOACTIVE),
        (ConsoleMode::NewConsoleVisible, WindowStartup::Hidden) => (CREATE_NEW_CONSOLE, SW_HIDE),
        // 隐藏新控制台：无窗口
        (ConsoleMode::NewConsoleHidden, _) => (CREATE_NO_WINDOW, SW_HIDE),
        // 完全脱离控制台
        (ConsoleMode::NoConsole, _) => (DETACHED_PROCESS, SW_HIDE),
    };
    ConsoleFlags { creation_flags: flag, show_window: show }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 控制台模式映射标志() {
        let visible = creation_flags(&ConsoleConfig {
            mode: ConsoleMode::NewConsoleVisible,
            startup: WindowStartup::Normal,
        });
        assert_eq!(visible.creation_flags, CREATE_NEW_CONSOLE);

        let hidden = creation_flags(&ConsoleConfig {
            mode: ConsoleMode::NewConsoleHidden,
            startup: WindowStartup::Normal,
        });
        assert_eq!(hidden.creation_flags, CREATE_NO_WINDOW);

        let detached = creation_flags(&ConsoleConfig {
            mode: ConsoleMode::NoConsole,
            startup: WindowStartup::Normal,
        });
        assert_eq!(detached.creation_flags, DETACHED_PROCESS);
    }
}
