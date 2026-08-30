//! GPU 占用采集（PDH "GPU Engine" / "GPU Process Memory" 性能计数器）
//!
//! - 利用率：进程树各引擎 `\GPU Engine(pid_N_*)\Utilization Percentage` 取最大值
//!   （与任务管理器"GPU"列同口径）
//! - 显存：`\GPU Process Memory(pid_N_*)\Local Usage` 求和（专用 GPU 内存）
//! - 计数器路径用英文名添加（PdhAddEnglishCounterW），中文等本地化系统不受影响
//! - 机器没有 WDDM GPU（部分虚拟机/远程会话）时计数器对象不存在，
//!   监控器进入不可用态，采样恒为 None，前端对应列隐藏

use std::collections::HashSet;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhEnumObjectItemsW,
    PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_NO_OBJECT,
    PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_MORE_DATA,
};

/// 计数器重建周期：GPU 引擎实例随进程开关 GPU 上下文动态增减，定期重建保持准确
const REBUILD_INTERVAL: Duration = Duration::from_secs(15);

/// 枚举缓冲上限（异常情况下防无限扩容）
const MAX_ENUM_BUF: u32 = 4 * 1024 * 1024;

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
    /// 引擎利用率计数器句柄
    engine_counters: Vec<isize>,
    /// GPU 进程内存计数器句柄
    mem_counters: Vec<isize>,
    /// 上次构建时的 PID 集（排序后，用于检测进程树变化）
    pids_key: Vec<u32>,
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
        let mut key: Vec<u32> = pids.to_vec();
        key.sort_unstable();
        let stale = self
            .built_at
            .is_none_or(|t| t.elapsed() >= REBUILD_INTERVAL);
        if (self.query == 0 || self.pids_key != key || stale) && !self.rebuild(&key) {
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
        // 显存：各实例求和
        let mut mem_bytes = 0u64;
        for &c in &self.mem_counters {
            if let Some(v) = formatted_double(c) {
                mem_bytes += v.max(0.0) as u64;
            }
        }
        // 利用率：各引擎取最大值
        if self.engine_counters.is_empty() {
            return Some(GpuSample {
                percent: 0.0,
                mem_bytes,
            });
        }
        let mut percent = 0.0f32;
        let mut any = false;
        for &c in &self.engine_counters {
            if let Some(v) = formatted_double(c) {
                percent = percent.max(v as f32);
                any = true;
            }
        }
        if !any {
            return None; // 全部计数器读取失败（实例刚消失等），视为本轮无数据
        }
        Some(GpuSample {
            percent: percent.clamp(0.0, 100.0),
            mem_bytes,
        })
    }

    /// 重建 PDH 查询：按 PID 集重新枚举并添加计数器。失败返回 false。
    fn rebuild(&mut self, key: &[u32]) -> bool {
        self.close();
        self.pids_key = key.to_vec();
        self.built_at = Some(Instant::now());
        self.primed = false;

        let mut query: isize = 0;
        if unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) } != 0 || query == 0 {
            return false;
        }
        self.query = query;

        // GPU Engine 对象不存在（无 WDDM GPU）→ 进入不可用态，之后不再尝试
        let instances = match enum_instances("GPU Engine") {
            Ok(list) => list,
            Err(PDH_CSTATUS_NO_OBJECT) => {
                self.unavailable = true;
                return false;
            }
            Err(_) => return false, // 其他错误视为暂时性，下一轮重试
        };
        let pids: HashSet<u32> = key.iter().copied().collect();
        for inst in &instances {
            let Some(pid) = instance_pid(inst) else {
                continue;
            };
            if !pids.contains(&pid) {
                continue;
            }
            add_counter(
                self.query,
                &format!("\\GPU Engine({inst})\\Utilization Percentage"),
                &mut self.engine_counters,
            );
        }
        // 显存对象缺失不致命：仅无显存数据
        if let Ok(list) = enum_instances("GPU Process Memory") {
            for inst in &list {
                let Some(pid) = instance_pid(inst) else {
                    continue;
                };
                if !pids.contains(&pid) {
                    continue;
                }
                add_counter(
                    self.query,
                    &format!("\\GPU Process Memory({inst})\\Local Usage"),
                    &mut self.mem_counters,
                );
            }
        }
        true
    }

    /// 关闭查询并清空计数器
    fn close(&mut self) {
        if self.query != 0 {
            unsafe { PdhCloseQuery(self.query) };
            self.query = 0;
        }
        self.engine_counters.clear();
        self.mem_counters.clear();
        self.primed = false;
    }
}

/// 读取计数器的格式化双精度值（无效数据返回 None）
fn formatted_double(counter: isize) -> Option<f64> {
    let mut v: PDH_FMT_COUNTERVALUE = unsafe { std::mem::zeroed() };
    let mut ty: u32 = 0;
    let rc = unsafe { PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, &mut ty, &mut v) };
    if rc != 0 {
        return None;
    }
    if v.CStatus != PDH_CSTATUS_VALID_DATA && v.CStatus != PDH_CSTATUS_NEW_DATA {
        return None;
    }
    Some(unsafe { v.Anonymous.doubleValue })
}

/// 枚举指定对象的实例名列表（多字符串缓冲按需扩容）
fn enum_instances(object: &str) -> Result<Vec<String>, u32> {
    let obj = to_wide(object);
    let mut counter_len: u32 = 0;
    let mut inst_len: u32 = 0;
    // 首次传空缓冲取所需长度
    let rc = unsafe {
        PdhEnumObjectItemsW(
            std::ptr::null(),
            std::ptr::null(),
            obj.as_ptr(),
            std::ptr::null_mut(),
            &mut counter_len,
            std::ptr::null_mut(),
            &mut inst_len,
            400, // PERF_DETAIL_WIZARD：列出全部实例
            0,
        )
    };
    if rc != 0 && rc != PDH_MORE_DATA {
        return Err(rc);
    }
    if inst_len == 0 {
        return Ok(Vec::new());
    }
    let mut counters = vec![0u16; counter_len.max(1) as usize];
    let mut instances = vec![0u16; inst_len.min(MAX_ENUM_BUF) as usize];
    let mut counter_len = counters.len() as u32;
    let mut inst_len = instances.len() as u32;
    let rc = unsafe {
        PdhEnumObjectItemsW(
            std::ptr::null(),
            std::ptr::null(),
            obj.as_ptr(),
            counters.as_mut_ptr(),
            &mut counter_len,
            instances.as_mut_ptr(),
            &mut inst_len,
            400,
            0,
        )
    };
    if rc != 0 && rc != PDH_MORE_DATA {
        return Err(rc);
    }
    Ok(parse_multi_sz(&instances))
}

/// 添加英文名计数器到查询（失败静默跳过该实例）
fn add_counter(query: isize, path: &str, out: &mut Vec<isize>) {
    let wide = to_wide(path);
    let mut h: isize = 0;
    if unsafe { PdhAddEnglishCounterW(query, wide.as_ptr(), 0, &mut h) } == 0 && h != 0 {
        out.push(h);
    }
}

/// GPU 引擎实例名 → PID（如 "pid_1234_luid_0x..._phys_0_eng_0"）
fn instance_pid(instance: &str) -> Option<u32> {
    let rest = instance.strip_prefix("pid_")?;
    let digits = rest.split('_').next()?;
    digits.parse().ok()
}

/// 解析双 NUL 结尾的多字符串缓冲
fn parse_multi_sz(buf: &[u16]) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, &c) in buf.iter().enumerate() {
        if c == 0 {
            if i == start {
                break; // 连续两个 NUL = 列表结束
            }
            out.push(String::from_utf16_lossy(&buf[start..i]));
            start = i + 1;
        }
    }
    out
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
    fn multi_sz解析() {
        let buf: Vec<u16> = "a\u{0}bc\u{0}\u{0}".encode_utf16().collect();
        assert_eq!(
            parse_multi_sz(&buf),
            vec!["a".to_string(), "bc".to_string()]
        );
        assert!(parse_multi_sz(&[]).is_empty());
    }

    #[test]
    fn 宽字符串带终止符() {
        let w = to_wide("\\GPU Engine");
        assert_eq!(w.last(), Some(&0));
        assert_eq!(w.len(), "\\GPU Engine".encode_utf16().count() + 1);
    }
}
