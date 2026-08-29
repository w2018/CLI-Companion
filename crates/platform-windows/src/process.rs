//! 进程指标采集：CPU 累计时间 + 内存工作集（服务资源监控用）
//!
//! 进程刚好退出或权限不足时返回 Err，调用方应静默跳过该次采样。

use std::io;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// 单进程瞬时指标快照
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcSnapshot {
    /// 内核 + 用户 CPU 累计时间（100ns 单位）
    pub cpu_time_100ns: u64,
    /// 物理内存工作集（字节）
    pub working_set_bytes: u64,
}

/// 读取指定 PID 的 CPU 累计时间与工作集内存
pub fn snapshot(pid: u32) -> io::Result<ProcSnapshot> {
    // 仅查询型权限：不要求同会话/同用户，对绝大多数受管服务可用
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let result = unsafe { read_snapshot(handle) };
    unsafe { CloseHandle(handle) };
    result
}

/// FILETIME（dwLow/dwHigh 两段 32 位）合并为 64 位 100ns 值
fn filetime_to_u64(ft: &windows_sys::Win32::Foundation::FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

unsafe fn read_snapshot(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> io::Result<ProcSnapshot> {
    let mut creation: windows_sys::Win32::Foundation::FILETIME = std::mem::zeroed();
    let mut exit: windows_sys::Win32::Foundation::FILETIME = std::mem::zeroed();
    let mut kernel: windows_sys::Win32::Foundation::FILETIME = std::mem::zeroed();
    let mut user: windows_sys::Win32::Foundation::FILETIME = std::mem::zeroed();
    if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    if GetProcessMemoryInfo(
        handle,
        &mut counters,
        std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
    ) == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(ProcSnapshot {
        cpu_time_100ns: filetime_to_u64(&kernel).saturating_add(filetime_to_u64(&user)),
        working_set_bytes: counters.WorkingSetSize as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 自身进程快照非零() {
        // 先消耗一段 CPU：刚启动的进程累计时间可能仍为 0
        let mut x = 1u64;
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_millis(30) {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        std::hint::black_box(x);
        let snap = snapshot(std::process::id()).expect("读取自身进程指标应成功");
        assert!(snap.cpu_time_100ns > 0);
        assert!(snap.working_set_bytes > 0);
    }

    #[test]
    fn 不存在的pid返回错误() {
        // 用一个几乎不可能存在的 PID 探测错误路径（Windows PID 一般 < 2^24 且不重复使用刚退出的）
        let r = snapshot(u32::MAX - 1);
        assert!(r.is_err());
    }
}
