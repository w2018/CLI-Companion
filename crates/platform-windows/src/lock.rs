//! 单例文件锁：保证同一数据目录只有一个 daemon 实例

use std::fs::OpenOptions;
use std::io;
use std::path::Path;

/// 已持有锁的 RAII 句柄；Drop 时释放 OS 锁
pub struct SingletonLock {
    _file: std::fs::File,
    path: std::path::PathBuf,
}

/// 锁被其他实例占用的错误
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("已有 daemon 实例在运行（锁被占用: {0}）")]
    AlreadyRunning(String),
    #[error("创建锁文件失败: {0}")]
    Io(#[from] io::Error),
}

impl SingletonLock {
    /// 尝试获取排他锁；被占用时返回 AlreadyRunning
    pub fn acquire(path: impl AsRef<Path>) -> Result<Self, LockError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(SingletonLock { _file: file, path }),
            Err(_) => Err(LockError::AlreadyRunning(path.display().to_string())),
        }
    }

    /// 锁文件路径（诊断用）
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 同一进程二次加锁失败() {
        let dir = std::env::temp_dir().join(format!("cli-comp-lock-test-{}", std::process::id()));
        let lock_path = dir.join("daemon.lock");
        let _g1 = SingletonLock::acquire(&lock_path).unwrap();
        let g2 = SingletonLock::acquire(&lock_path);
        assert!(matches!(g2, Err(LockError::AlreadyRunning(_))));
        drop(_g1);
        // 释放后可重新获取
        let _g3 = SingletonLock::acquire(&lock_path).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
