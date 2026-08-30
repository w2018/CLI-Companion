//! TCP 连接级流量统计（服务网络速率采集用）
//!
//! 口径：系统 TCP 连接表（GetExtendedTcpTable，v4+v6）定位进程树的连接，
//! 逐连接读取字节统计（GetPerTcpConnectionEStats）。
//!
//! 关键事实：TCP_ESTATS Data 路径统计**默认不采集**，未启用的连接返回
//! 无意义数值（实测可达 10^16 量级）。因此采集器对每个新连接先
//! SetPerTcpConnectionEStats 启用统计、再以启用后的首次读数为基线，
//! 只对已跟踪的连接做窗口差分。
//!
//! 已知口径限制：UDP / ICMP 流量不计入；连接被发现前的窗口与其存续
//! 期间最后一次采样的残余字节会漏计。

use std::collections::{HashMap, HashSet};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, GetPerTcp6ConnectionEStats, GetPerTcpConnectionEStats,
    SetPerTcp6ConnectionEStats, SetPerTcpConnectionEStats, TCP_ESTATS_DATA_ROD_v0,
    TCP_ESTATS_DATA_RW_v0, TcpConnectionEstatsData, MIB_TCP6ROW, MIB_TCP6TABLE_OWNER_MODULE,
    MIB_TCPROW_LH, MIB_TCPTABLE_OWNER_MODULE, TCP_TABLE_OWNER_MODULE_ALL,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

/// 单连接单窗口增量的合理性上限（防内核计数异常造成天文数字）
const MAX_PER_CONN_WINDOW: u64 = 4 * 1024 * 1024 * 1024;

/// 参与统计的连接状态范围：ESTAB(5) 到 TIME_WAIT(11)。
/// 必须包含收尾态（FIN_WAIT2 / CLOSE_WAIT 等）：传输完立即关闭的连接
/// 在采样时往往已在收尾，仅统计 ESTAB 会永久丢失其字节；这些状态下
/// 计数器冻结可读，TIME_WAIT 还能让关闭后的最后一波字节入账。
const STATE_MIN: u32 = 5;
const STATE_MAX: u32 = 11;

/// 一个采样窗口内进程树 TCP 收发字节增量
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TcpDelta {
    /// 本窗口接收字节增量（各连接 DataBytesIn 差分之和）
    pub in_bytes: u64,
    /// 本窗口发送字节增量（各连接 DataBytesOut 差分之和）
    pub out_bytes: u64,
}

/// TCP 流量采集器（每服务一个，actor 串行使用）
///
/// 维护"连接 → 上次累计字节"基线；对基线表中没有的连接先启用统计。
/// 基线在读到统计值之后建立，因此未启用期的历史字节不会混入差分。
#[derive(Default)]
pub struct NetMonitor {
    /// (本地端口, 远程端口) → 上次累计（收, 发）原始字节
    baselines: HashMap<(u32, u32), (u64, u64)>,
}

impl NetMonitor {
    /// 清空基线（服务停止/重启后旧连接全部失效）
    pub fn reset(&mut self) {
        self.baselines.clear();
    }

    /// 采样一个窗口：返回各已跟踪连接的本窗口收发字节增量之和
    ///
    /// 新连接本窗口计 0（完成启用 + 建基线），下一个窗口起计入。
    pub fn sample(&mut self, pids: &[u32]) -> TcpDelta {
        let mut delta = TcpDelta::default();
        if pids.is_empty() {
            self.baselines.clear();
            return delta;
        }
        let set: HashSet<u32> = pids.iter().copied().collect();
        let mut seen: HashSet<(u32, u32)> = HashSet::new();

        // ===== IPv4 =====
        if let Some(buf) = query_raw_table(AF_INET as u32) {
            let table = unsafe { &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_MODULE) };
            let rows = unsafe {
                std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize)
            };
            for row in rows {
                let st = row.dwState;
                if !set.contains(&row.dwOwningPid) || !(STATE_MIN..=STATE_MAX).contains(&st) {
                    continue;
                }
                let key = (row.dwLocalPort, row.dwRemotePort);
                seen.insert(key);
                // MIB_TCPROW_OWNER_MODULE 与 MIB_TCPROW_LH 前缀字段布局一致
                // （dwState/local/remote 五元组），可安全转型
                let row_ptr = row as *const _ as *const MIB_TCPROW_LH;
                match self.baselines.get(&key) {
                    Some(&(pin, pout)) => {
                        let mut rod: TCP_ESTATS_DATA_ROD_v0 = unsafe { std::mem::zeroed() };
                        if read_v4(row_ptr, &mut rod) != 0 {
                            continue;
                        }
                        let cur = (rod.DataBytesIn, rod.DataBytesOut);
                        delta.in_bytes += cur.0.saturating_sub(pin).min(MAX_PER_CONN_WINDOW);
                        delta.out_bytes += cur.1.saturating_sub(pout).min(MAX_PER_CONN_WINDOW);
                        self.baselines.insert(key, cur);
                    }
                    None => {
                        // 新连接：先启用 Data 统计（计数被重置），再读数建基线；
                        // 基线是启用后的读数，未启用期的历史字节不混入后续差分
                        enable_v4(row_ptr);
                        let mut rod: TCP_ESTATS_DATA_ROD_v0 = unsafe { std::mem::zeroed() };
                        let cur = if read_v4(row_ptr, &mut rod) == 0 {
                            (rod.DataBytesIn, rod.DataBytesOut)
                        } else {
                            (0, 0)
                        };
                        self.baselines.insert(key, cur);
                    }
                }
            }
        }

        // ===== IPv6 =====
        if let Some(buf) = query_raw_table(AF_INET6 as u32) {
            let table = unsafe { &*(buf.as_ptr() as *const MIB_TCP6TABLE_OWNER_MODULE) };
            let rows = unsafe {
                std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize)
            };
            for row in rows {
                let st = row.dwState;
                if !set.contains(&row.dwOwningPid) || !(STATE_MIN..=STATE_MAX).contains(&st) {
                    continue;
                }
                let key = (row.dwLocalPort, row.dwRemotePort);
                seen.insert(key);
                // MIB_TCP6ROW_OWNER_MODULE 与 MIB_TCP6ROW 前缀字段布局一致
                let row_ptr = row as *const _ as *const MIB_TCP6ROW;
                match self.baselines.get(&key) {
                    Some(&(pin, pout)) => {
                        let mut rod: TCP_ESTATS_DATA_ROD_v0 = unsafe { std::mem::zeroed() };
                        if read_v6(row_ptr, &mut rod) != 0 {
                            continue;
                        }
                        let cur = (rod.DataBytesIn, rod.DataBytesOut);
                        delta.in_bytes += cur.0.saturating_sub(pin).min(MAX_PER_CONN_WINDOW);
                        delta.out_bytes += cur.1.saturating_sub(pout).min(MAX_PER_CONN_WINDOW);
                        self.baselines.insert(key, cur);
                    }
                    None => {
                        enable_v6(row_ptr);
                        let mut rod: TCP_ESTATS_DATA_ROD_v0 = unsafe { std::mem::zeroed() };
                        let cur = if read_v6(row_ptr, &mut rod) == 0 {
                            (rod.DataBytesIn, rod.DataBytesOut)
                        } else {
                            (0, 0)
                        };
                        self.baselines.insert(key, cur);
                    }
                }
            }
        }

        // 已消失的连接移出基线表（其最后窗口的残余增量按口径放弃）
        self.baselines.retain(|k, _| seen.contains(k));
        delta
    }
}

/// 读取 IPv4 连接的 Data 路径统计（成功返回 0）
fn read_v4(row_ptr: *const MIB_TCPROW_LH, rod: &mut TCP_ESTATS_DATA_ROD_v0) -> u32 {
    unsafe {
        GetPerTcpConnectionEStats(
            row_ptr,
            TcpConnectionEstatsData,
            std::ptr::null_mut(),
            0,
            0,
            std::ptr::null_mut(),
            0,
            0,
            rod as *mut _ as *mut u8,
            0,
            std::mem::size_of::<TCP_ESTATS_DATA_ROD_v0>() as u32,
        )
    }
}

/// 读取 IPv6 连接的 Data 路径统计（成功返回 0）
fn read_v6(row_ptr: *const MIB_TCP6ROW, rod: &mut TCP_ESTATS_DATA_ROD_v0) -> u32 {
    unsafe {
        GetPerTcp6ConnectionEStats(
            row_ptr,
            TcpConnectionEstatsData,
            std::ptr::null_mut(),
            0,
            0,
            std::ptr::null_mut(),
            0,
            0,
            rod as *mut _ as *mut u8,
            0,
            std::mem::size_of::<TCP_ESTATS_DATA_ROD_v0>() as u32,
        )
    }
}

/// 为 IPv4 连接启用 Data 路径统计（失败静默：后续窗口读数差分为 0）
fn enable_v4(row_ptr: *const MIB_TCPROW_LH) {
    let rw = TCP_ESTATS_DATA_RW_v0 {
        EnableCollection: 1,
    };
    unsafe {
        SetPerTcpConnectionEStats(
            row_ptr,
            TcpConnectionEstatsData,
            &rw as *const _ as *const u8,
            0,
            std::mem::size_of::<TCP_ESTATS_DATA_RW_v0>() as u32,
            0,
        )
    };
}

/// 为 IPv6 连接启用 Data 路径统计
fn enable_v6(row_ptr: *const MIB_TCP6ROW) {
    let rw = TCP_ESTATS_DATA_RW_v0 {
        EnableCollection: 1,
    };
    unsafe {
        SetPerTcp6ConnectionEStats(
            row_ptr,
            TcpConnectionEstatsData,
            &rw as *const _ as *const u8,
            0,
            std::mem::size_of::<TCP_ESTATS_DATA_RW_v0>() as u32,
            0,
        )
    };
}

/// 查询原始连接表缓冲（自动按需扩容，失败返回 None）
fn query_raw_table(ulaf: u32) -> Option<Vec<u8>> {
    let mut size: u32 = 16 * 1024;
    for _ in 0..4 {
        let mut buf = vec![0u8; size as usize];
        let rc = unsafe {
            GetExtendedTcpTable(
                buf.as_mut_ptr() as *mut _,
                &mut size,
                0,
                ulaf,
                TCP_TABLE_OWNER_MODULE_ALL,
                0,
            )
        };
        match rc {
            0 => return Some(buf),
            // 122 = ERROR_INSUFFICIENT_BUFFER，234 = ERROR_MORE_DATA：size 已更新为所需大小
            122 | 234 => continue,
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn 连接采集器回环增量准确() {
        let mut mon = NetMonitor::default();
        let pids = [std::process::id()];

        // 先建立连接，再采基线窗口（新连接窗口只建基线不计增量）
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定回环端口应成功");
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("accept 应成功");
            let mut buf = [0u8; 8192];
            let mut echoed = 0usize;
            while echoed < 256 * 1024 {
                let n = sock.read(&mut buf).expect("读应成功");
                if n == 0 {
                    break;
                }
                sock.write_all(&buf[..n]).expect("回写应成功");
                echoed += n;
            }
        });
        let mut client = TcpStream::connect(addr).expect("连接应成功");

        let d0 = mon.sample(&pids);
        assert_eq!(d0.in_bytes + d0.out_bytes, 0, "基线窗口增量应为 0");

        // 真实回环 TCP 双向传输 256KB
        let payload = vec![7u8; 256 * 1024];
        client.write_all(&payload).expect("发送应成功");
        let mut received = 0usize;
        let mut buf = [0u8; 8192];
        while received < 256 * 1024 {
            let n = client.read(&mut buf).expect("读回显应成功");
            if n == 0 {
                break;
            }
            received += n;
        }
        assert_eq!(received, 256 * 1024, "回显数据应完整");

        // 传输后的窗口：收发增量应落在真实字节量级（连接可能已进入收尾态）
        let d1 = mon.sample(&pids);
        assert!(
            d1.out_bytes >= 256 * 1024,
            "发送增量应 ≥ 256KB: {}",
            d1.out_bytes
        );
        assert!(
            d1.in_bytes >= 256 * 1024,
            "接收增量应 ≥ 256KB: {}",
            d1.in_bytes
        );
        assert!(
            d1.in_bytes + d1.out_bytes < 2 * 1024 * 1024,
            "收发增量之和不应显著超过实际传输量（防垃圾计数）: {d1:?}"
        );

        // 无流量窗口：增量归零
        let d2 = mon.sample(&pids);
        assert_eq!(d2.in_bytes + d2.out_bytes, 0, "空闲窗口增量应为 0");

        // 连接关闭后：增量保持 0（TIME_WAIT 行可能仍留在基线表，属正常）
        drop(client);
        handle.join().expect("回显线程不应 panic");
        let d3 = mon.sample(&pids);
        assert_eq!(d3.in_bytes + d3.out_bytes, 0, "连接关闭后增量应为 0");
    }

    #[test]
    fn reset清空基线() {
        let mut mon = NetMonitor::default();
        let _ = mon.sample(&[std::process::id()]);
        mon.reset();
        assert!(mon.baselines.is_empty());
    }
}
