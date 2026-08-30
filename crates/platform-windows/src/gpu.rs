//! GPU 占用采集（PDH "GPU Engine" / "GPU Process Memory" 性能计数器）
//!
//! - 利用率：进程树各引擎 `\GPU Engine(*)\Utilization Percentage` 中
//!   属于本树的实例取最大值（与任务管理器"GPU"列同口径）
//! - 显存：`\GPU Process Memory(*)\Local Usage` 属于本树的实例求和（专用 GPU 内存）
//! - 计数器以**英文名通配路径**添加（PdhAddEnglishCounterW），采样时读计数器
//!   数组、按实例名的 `pid_N_` 前缀过滤进程树——全程不经 PdhEnumObjectItemsW，
//!   规避其在非英文系统上需要本地化对象名的问题（实测中文系统英文枚举失败）
//! - 机器没有 WDDM GPU（部分虚拟机/远程会话）时计数器对象不存在，
//!   监控器进入不可用态，采样恒为 None，前端对应列隐藏

use std::collections::HashSet;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_NO_OBJECT, PDH_CSTATUS_VALID_DATA,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
};

/// 计数器重建周期：通配计数器在添加时展开实例，需定期重建以纳入新实例
const REBUILD_INTERVAL: Duration = Duration::from_secs(15);

/// 单次 GPU 采样结果
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuSample {
    /// GPU 利用率（0-100，进程树各引擎取最大值）
    pub percent: f32,
    /// 专用 GPU 内存（字节，各实例 Local Usage 求和）
    pub mem_bytes: u64,
}

/// 每服务 GPU 监控器（持有 PDH 查询句柄，仅供 actor 串行使用，不跨线程共享）
#[derive(Default)]
pub struct GpuMonitor {
    /// PDH 查询句柄（0 = 未打开）
    query: isize,
    /// 引擎利用率通配计数器（0 = 未添加）
    eng_counter: isize,
    /// GPU 进程内存通配计数器（0 = 未添加/对象不存在）
    mem_counter: isize,
    /// 上次构建时刻
    built_at: Option<Instant>,
    /// GPU 计数器对象不可用（无 GPU / 虚拟机），永久跳过采样
    unavailable: bool,
    /// 重建后是否已首采（利用率类计数器需两次采集才有值）
    primed: bool,
}

impl Drop for GpuMonitor {
    fn drop(&mut self) {
        self.close();
    }
}

impl GpuMonitor {
    /// 采样一次。None 表示本轮无有效数据（首采建基线 / 重建中 / 机器无 GPU），
    /// 调用方沿用上次值。进程树内没有 GPU 实例时返回 0%（与任务管理器一致）。
    pub fn sample(&mut self, pids: &[u32]) -> Option<GpuSample> {
        if self.unavailable {
            return None;
        }
        let stale = self
            .built_at
            .is_none_or(|t| t.elapsed() >= REBUILD_INTERVAL);
        if (self.query == 0 || stale) && !self.rebuild() {
            return None;
        }
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            // 采集失败：丢弃查询，下一轮重建
            self.close();
            return None;
        }
        if !self.primed {
            self.primed = true;
            return None; // 首采仅建立基线，利用率尚无值
        }
        let pids: HashSet<u32> = pids.iter().copied().collect();
        // 利用率：各引擎实例取最大值（实例名带 pid_N_ 前缀，按进程树过滤）
        let mut percent = 0.0f32;
        for (name, v) in read_counter_array(self.eng_counter) {
            if instance_pid(&name).is_some_and(|p| pids.contains(&p)) {
                percent = percent.max(v as f32);
            }
        }
        // 显存：各实例求和
        let mut mem_bytes = 0u64;
        if self.mem_counter != 0 {
            for (name, v) in read_counter_array(self.mem_counter) {
                if instance_pid(&name).is_some_and(|p| pids.contains(&p)) {
                    mem_bytes += v.max(0.0) as u64;
                }
            }
        }
        Some(GpuSample {
            percent: percent.clamp(0.0, 100.0),
            mem_bytes,
        })
    }

    /// 重建 PDH 查询：重新添加通配计数器以纳入新出现的 GPU 实例。失败返回 false。
    fn rebuild(&mut self) -> bool {
        self.close();
        self.built_at = Some(Instant::now());
        self.primed = false;

        let mut query: isize = 0;
        if unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) } != 0 || query == 0 {
            return false;
        }
        self.query = query;

        // GPU Engine 对象不存在（无 WDDM GPU）→ 进入不可用态，之后不再尝试
        match add_english_counter(
            query,
            "\\GPU Engine(*)\\Utilization Percentage",
            PDH_CSTATUS_NO_OBJECT,
        ) {
            Some(h) => self.eng_counter = h,
            None => return false,
        }
        // 显存对象缺失不致命：仅无显存数据（部分驱动不提供该对象）
        self.mem_counter =
            add_english_counter(query, "\\GPU Process Memory(*)\\Local Usage", 0).unwrap_or(0);
        true
    }

    /// 关闭查询并清空计数器
    fn close(&mut self) {
        if self.query != 0 {
            unsafe { PdhCloseQuery(self.query) };
            self.query = 0;
        }
        self.eng_counter = 0;
        self.mem_counter = 0;
        self.primed = false;
    }
}

/// 向查询添加英文名计数器；返回计数器句柄（0 = 未添加）。
///
/// `permanent_fail` 非 0 时：该错误码视为对象级缺失（监控器进入不可用态），
/// 返回 None；其余错误仅本轮失败，下一轮重试。
fn add_english_counter(query: isize, path: &str, permanent_fail: u32) -> Option<isize> {
    let wide = to_wide(path);
    let mut h: isize = 0;
    let rc = unsafe { PdhAddEnglishCounterW(query, wide.as_ptr(), 0, &mut h) };
    if rc == 0 && h != 0 {
        return Some(h);
    }
    if permanent_fail != 0 && rc == permanent_fail {
        return None;
    }
    Some(0)
}

/// 读取多实例计数器的格式化值数组（实例名, 双精度值），失败返回空表
fn read_counter_array(counter: isize) -> Vec<(String, f64)> {
    if counter == 0 {
        return Vec::new();
    }
    let mut size: u32 = 64 * 1024;
    for _ in 0..4 {
        // 缓冲按 8 字节对齐分配（条目含指针与 f64）
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let mut count: u32 = 0;
        let rc = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W,
            )
        };
        match rc {
            0 => {
                let items = unsafe {
                    std::slice::from_raw_parts(
                        buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W,
                        count as usize,
                    )
                };
                return items
                    .iter()
                    .filter(|it| {
                        it.FmtValue.CStatus == PDH_CSTATUS_VALID_DATA
                            || it.FmtValue.CStatus == PDH_CSTATUS_NEW_DATA
                    })
                    .map(|it| {
                        (pwstr_to_string(it.szName), unsafe {
                            it.FmtValue.Anonymous.doubleValue
                        })
                    })
                    .collect();
            }
            PDH_MORE_DATA => continue, // size 已更新为所需大小
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

/// GPU 引擎实例名 → PID（如 "pid_1234_luid_0x..._phys_0_eng_0"）
fn instance_pid(instance: &str) -> Option<u32> {
    let rest = instance.strip_prefix("pid_")?;
    let digits = rest.split('_').next()?;
    digits.parse().ok()
}

/// NUL 结尾的 UTF-16 指针 → String
fn pwstr_to_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
        }
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) })
}

/// UTF-8 → NUL 结尾的 UTF-16
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu采样结果在界内或不可用() {
        let mut m = GpuMonitor::default();
        let pids = vec![std::process::id()];
        let _ = m.sample(&pids); // 首轮：重建 + 建基线，应为 None
        std::thread::sleep(Duration::from_millis(1100));
        let second = m.sample(&pids);
        // 无 GPU 的机器（部分虚拟机/远程会话）全程 None 也合法；
        // 有 GPU 时利用率必须落在 0-100
        if let Some(s) = second {
            assert!(
                (0.0..=100.0).contains(&s.percent),
                "GPU 利用率越界: {}",
                s.percent
            );
        }
    }

    #[test]
    fn 实例名解析pid() {
        assert_eq!(
            instance_pid("pid_1234_luid_0x00000000_phys_0_eng_0"),
            Some(1234)
        );
        assert_eq!(instance_pid("pid_42_phys_0"), Some(42));
        assert_eq!(instance_pid("system_idle"), None);
        assert_eq!(instance_pid("pid_"), None);
    }

    #[test]
    fn 宽字符串往返() {
        let w = to_wide("\\GPU Engine");
        assert_eq!(w.last(), Some(&0));
        assert_eq!(pwstr_to_string(w.as_ptr()), "\\GPU Engine");
        assert_eq!(pwstr_to_string(std::ptr::null()), "");
    }
}
