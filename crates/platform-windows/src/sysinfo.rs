//! 系统级信息采集：物理内存总量（内存占用百分比的分母）

use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

/// 系统物理内存总量（字节）；读取失败返回 0（调用方据此跳过百分比计算）
pub fn total_phys_bytes() -> u64 {
    let mut ms: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    ms.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut ms) } == 0 {
        return 0;
    }
    ms.ullTotalPhys
}

#[cfg(test)]
mod tests {
    #[test]
    fn 物理内存总量合理() {
        let total = super::total_phys_bytes();
        assert!(total > 1024 * 1024 * 1024, "物理内存应大于 1 GB：{total}");
    }
}
