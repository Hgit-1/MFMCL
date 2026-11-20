//! Cross-process locks. Keep lock files in place: unlinking one while it is
//! locked could let another process open a different inode at the same path.
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};

pub(crate) const GAME_LOCK_FILE: &str = ".mfmcl-game.lock";

fn process_start(pid: u32) -> Option<u64> {
    let pid = sysinfo::Pid::from_u32(pid);
    let mut system = sysinfo::System::new();
    system.refresh_process(pid);
    system
        .process(pid)
        .filter(|p| {
            !matches!(
                p.status(),
                sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
            )
        })
        .map(sysinfo::Process::start_time)
}

#[derive(Debug)]
pub struct OperationLock {
    _file: Option<File>,
}

impl OperationLock {
    pub(crate) fn in_memory() -> Self {
        Self { _file: None }
    }

    /// A shared lock excludes recovery; an exclusive lock excludes other owners.
    pub(crate) fn try_acquire(path: &Path, shared: bool) -> io::Result<Option<Self>> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let result = if shared {
            file.try_lock_shared()
        } else {
            file.try_lock()
        };
        match result {
            Ok(()) => Ok(Some(Self { _file: Some(file) })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }
}

/// A game-directory lock, shared by desktop and CLI and independent of instance
/// ID and app data root. Alternate paths to the same directory open the same lock.
#[derive(Debug)]
pub struct GameDirectoryLock {
    _guard: OperationLock,
    directory: PathBuf,
}

impl GameDirectoryLock {
    pub fn acquire(directory: &Path) -> Result<Self, String> {
        let directory = directory
            .canonicalize()
            .map_err(|error| format!("游戏目录不可用：{error}"))?;
        let guard = OperationLock::try_acquire(&directory.join(GAME_LOCK_FILE), false)
            .map_err(|error| format!("无法锁定游戏目录：{error}"))?
            .ok_or("此游戏目录正在被游戏或备份任务使用，请等待当前操作结束。")?;
        let lock = Self {
            _guard: guard,
            directory,
        };
        lock.check_previous_game()?;
        Ok(lock)
    }

    fn check_previous_game(&self) -> Result<(), String> {
        let file = self._guard._file.as_ref().expect("game file lock");
        let mut text = String::new();
        file.take(96)
            .read_to_string(&mut text)
            .map_err(|error| error.to_string())?;
        if text.is_empty() {
            return Ok(());
        }
        let (pid, start) = text
            .trim()
            .split_once(' ')
            .and_then(|(pid, start)| Some((pid.parse::<u32>().ok()?, start.parse::<u64>().ok()?)))
            .ok_or("无法读取上次游戏的进程记录，请先确认游戏已经退出。")?;
        if process_start(pid).is_some_and(|current| start == 0 || current == start) {
            return Err("此游戏目录仍被上次启动的游戏占用，请先退出游戏。".into());
        }
        Ok(())
    }

    /// Retain the process identity after the launcher exits (e.g. CLI --detach).
    pub fn record_game_process(&self, pid: u32) -> Result<(), String> {
        let mut file = self._guard._file.as_ref().expect("game file lock");
        let start = process_start(pid).unwrap_or(0);
        let result = (|| -> io::Result<()> {
            file.rewind()?;
            file.set_len(0)?;
            writeln!(file, "{pid} {start}")?;
            file.sync_data()
        })();
        if let Err(error) = result {
            let _ = file.set_len(0);
            return Err(format!("无法记录正在运行的游戏：{error}"));
        }
        Ok(())
    }

    pub(crate) fn matches(&self, directory: &Path) -> bool {
        directory
            .canonicalize()
            .is_ok_and(|path| path == self.directory)
    }
}
