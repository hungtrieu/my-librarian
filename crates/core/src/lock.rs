//! Khoá chống quét chồng giữa app và tác vụ ngầm (E8-03). Khoá tự nhả khi tiến trình thoát.

use crate::error::{Error, Result};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

pub struct ScanLock {
    _file: File,
}

impl ScanLock {
    pub fn try_acquire(home: &Path) -> Result<ScanLock> {
        std::fs::create_dir_all(home)?;
        let f = OpenOptions::new().create(true).truncate(false).write(true).open(home.join("scan.lock"))?;
        match f.try_lock() {
            Ok(()) => Ok(ScanLock { _file: f }),
            Err(TryLockError::WouldBlock) => Err(Error::Locked),
            Err(TryLockError::Error(e)) => Err(e.into()),
        }
    }
}
