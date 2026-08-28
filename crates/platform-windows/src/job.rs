//! Job Object：将受管服务进程树纳入管理单元
//!
//! - KILL_ON_JOB_CLOSE：daemon 异常退出时自动清理全部子进程
//! - TerminateJobObject：停止服务时清理整个进程树，不留孤儿

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// RAII 持有的 Job Object 句柄
pub struct Job(windows_sys::Win32::Foundation::HANDLE);

// HANDLE 是裸指针，但 Job 对象句柄可跨线程使用（Windows 内核对象）
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    /// 创建带 KILL_ON_JOB_CLOSE 限制的 Job
    pub fn create() -> io::Result<Self> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                let err = io::Error::last_os_error();
                CloseHandle(handle);
                return Err(err);
            }
            Ok(Job(handle))
        }
    }

    /// 将子进程关联到 Job（其后代进程默认也进入同一 Job）
    pub fn assign(&self, child: &Child) -> io::Result<()> {
        unsafe {
            let ok = AssignProcessToJobObject(self.0, child.as_raw_handle());
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    /// 终止 Job 内全部进程（强制杀，作为优雅停止失败后的最后手段）
    pub fn terminate(&self) -> io::Result<()> {
        unsafe {
            let ok = TerminateJobObject(self.0, 1);
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // 关闭句柄触发 KILL_ON_JOB_CLOSE，清理残留子进程
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 创建job并可终止() {
        let job = Job::create().unwrap();
        // 空_job 终止不应报错
        job.terminate().unwrap();
    }
}
