//! 启动器自身的日志文件。
//!
//! ## 为什么需要它
//!
//! 游戏日志（`logs/latest.log`）只覆盖游戏进程启动之后的事。用户遇到的大部分问题
//! 发生在更早：安装失败、Java 没探测到、下载中断、插件被跳过、登录被拒。这些
//! 只在启动器进程里，用户想报 issue 时手里什么都没有。
//!
//! 所以这里把启动器自己的事件写进一个固定文件，用户点一下就能拿到它。
//!
//! ## 三条约束
//!
//! 1. **必须脱敏后落盘。** 启动器接触 access token、refresh token、账号名。日志
//!    要被贴到公开的 issue 里，任何一条明文凭据都是一次泄漏。写入前统一过
//!    `logs::redact_log`，与诊断包导出用的是同一套规则——两处规则一旦分叉，
//!    泄漏就会从较松的那一处发生。
//! 2. **必须有上限。** 一个长期运行的启动器会积累到几百 MB。超过上限就滚动：
//!    保留一份 `.1` 备份，其余丢弃。报 issue 需要的是"最近发生了什么"，不是
//!    完整历史。
//! 3. **写日志失败不能影响功能。** 磁盘满、目录只读、权限被拒——这些都只让这条
//!    日志少一行，绝不能让启动、安装、保存设置失败。
//!
//! ## 默认关闭
//!
//! 这是**用户主动开启**的开关，不是默认行为。尽管内容已脱敏，把本机操作序列写成
//! 文件仍是用户应当知情并同意的决定。开启后界面会说明文件在哪、怎么提交 issue。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 单个日志文件的上限。
///
/// 2 MB 对纯文本是几万行，足够覆盖一次完整的「启动 → 崩溃 → 重试」。再大对报
/// issue 没有帮助：贴 50 MB 的日志没人会看完。
const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// 滚动后保留的备份数量。只留 1 份：需要更早的记录时应当让用户复现一次。
const MAX_ROTATED: u32 = 1;

/// 一条日志的级别。
///
/// 只有三级：这里记录的是「用户报问题时需要看的事件」，不是调试插桩。级别多了
/// 会让用户以为调高就能看到更多——真正需要细粒度追踪时应当用专门的调试构建。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

/// 启动器日志写入器。
///
/// 用 `Mutex<Option<...>>` 而不是每次打开文件：日志是高频追加，反复 open/close
/// 既慢又会在文件被别的程序占用时失败。`Option` 表示「当前是否开启」。
#[derive(Default)]
pub struct LauncherLog {
    inner: Mutex<Option<Writer>>,
}

struct Writer {
    file: std::fs::File,
    path: PathBuf,
    written: u64,
}

impl LauncherLog {
    /// 打开（或重新打开）日志文件。
    ///
    /// 用追加模式而不是覆盖：上一次会话的末尾往往是问题发生的地方，覆盖掉会让
    /// 用户「重启一次试试」之后丢掉证据。
    pub fn enable(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("无法创建日志目录：{error}"))?;
        }
        Self::rotate_if_needed(path);
        let file = open_private(path)?;
        let written = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        let mut guard = self.inner.lock().map_err(|_| "日志状态不可用。")?;
        *guard = Some(Writer {
            file,
            path: path.to_path_buf(),
            written,
        });
        Ok(())
    }

    /// 关闭日志。之后的写入会被静默忽略。
    pub fn disable(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = None;
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.inner
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    /// 写一行。
    ///
    /// 返回 `()`：调用方永远不需要处理日志写入失败。这是刻意的——把日志失败
    /// 冒泡出去，会让「磁盘满了」变成「启动失败」，代价远大于少一行记录。
    pub fn write(&self, level: Level, message: &str) {
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        let Some(writer) = guard.as_mut() else {
            return;
        };
        /*
         * 脱敏在写入前做，且对**每一条**都做。
         *
         * 不做「只对看起来像凭据的字段脱敏」——那要求每个调用点都判断得对，而
         * 一次判断失误就是一次泄漏。统一过 `redact_log` 的代价只是每行一次正则，
         * 换的是「任何调用点写错也不会泄漏」。
         */
        let safe = crate::logs::redact_log(message, &[]);
        let stamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let line = format!("[{stamp}] {:<5} {safe}\n", level.as_str());

        if writer.written + line.len() as u64 > MAX_BYTES {
            /*
             * 到上限就滚动：把当前文件改名成 `.1`，再新建一个空的。
             *
             * 顺序是「先改名、再新建」，而不是「先截断、再改名」——后者会让旧的
             * 内容在改名之前就没了，等于什么都没保留。
             *
             * 任何一步失败都只是让这次滚动不生效（日志继续往旧文件里写，直到下
             * 一次尝试），不会影响调用方。
             */
            let path = writer.path.clone();
            Self::rotate_files(&path);
            match open_private(&path) {
                Ok(file) => {
                    writer.file = file;
                    writer.written = 0;
                }
                Err(_) => return,
            }
        }
        if writer.file.write_all(line.as_bytes()).is_err() {
            // 写失败通常意味着磁盘已满或文件被删除。关掉，避免每行都失败一次。
            *guard = None;
            return;
        }
        writer.written = writer.written.saturating_add(line.len() as u64);
    }

    /// 把当前文件滚到 `.1`（覆盖旧的 `.1`）。只保留一份轮转。
    fn rotate_if_needed(path: &Path) {
        let oversized = std::fs::metadata(path).is_ok_and(|meta| meta.len() >= MAX_BYTES);
        if oversized {
            Self::rotate_files(path);
        }
    }

    fn rotate_files(path: &Path) {
        if MAX_ROTATED == 0 {
            return;
        }
        let backup = rotated_path(path, 1);
        // 先删旧的再改名：Windows 上 rename 到已存在的文件会失败。
        let _ = std::fs::remove_file(&backup);
        let _ = std::fs::rename(path, &backup);
    }
}

/// 以「只有本人可读写」的权限打开日志文件。
///
/// ## 为什么不能直接用 `File::create` / `OpenOptions`
///
/// 它们按进程的 umask 创建文件，在常见的 `umask 002` 下会得到 `0o664`——同机的
/// 其他用户能读到。日志里有实例名、文件路径、错误原文，这些对报 issue 有必要，
/// 对别人则没有必要。
///
/// 测试 `log_file_is_private_to_the_owner` 抓到过这个问题：早期实现用的是普通
/// `OpenOptions`，权限实际是 664。
///
/// ## 为什么写完再 chmod 不行
///
/// 那中间存在一个「文件已存在、权限还是宽的」窗口，同机的进程可以在那一瞬间读完
/// 整份日志。用 `mode` 参数在 `open(2)` 里一次定好，就没有这个窗口。
fn open_private(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // 已存在的文件不会被这个 mode 改动，所以下面还要再显式收紧一次。
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .map_err(|error| format!("无法打开日志文件：{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 覆盖「文件是旧版本建的、权限还是 664」这种情况。
        let current = file
            .metadata()
            .map(|meta| meta.permissions().mode() & 0o777)
            .unwrap_or(0);
        if current != 0o600 {
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
    }
    Ok(file)
}

fn rotated_path(path: &Path, index: u32) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{index}"));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mfmcl-log-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_readable_lines_with_level_and_timestamp() {
        let dir = temp_dir();
        let path = dir.join("launcher.log");
        let log = LauncherLog::default();
        log.enable(&path).unwrap();
        log.write(Level::Info, "启动器已启动");
        log.write(Level::Error, "下载失败");
        log.disable();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("INFO"), "{content}");
        assert!(content.contains("ERROR"), "{content}");
        assert!(content.contains("启动器已启动"), "{content}");
        assert!(content.contains("下载失败"), "{content}");
        // 每行都带时间戳，便于把日志和别处的事件对上。
        assert!(
            content.lines().all(|line| line.starts_with('[')),
            "{content}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 凭据绝不能出现在文件里。
    ///
    /// 这不是理论风险：启动器的日志调用点会记录完整的错误信息，而认证失败的错误
    /// 里经常带着请求体片段。日志文件是要被贴到公开 issue 的。
    #[test]
    fn credentials_are_redacted_before_hitting_disk() {
        let dir = temp_dir();
        let path = dir.join("launcher.log");
        let log = LauncherLog::default();
        log.enable(&path).unwrap();
        log.write(
            Level::Error,
            "login failed: access_token=super-secret-value",
        );
        log.write(Level::Error, "{\"refreshToken\":\"another-secret\"}");
        log.disable();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("super-secret-value"), "leaked: {content}");
        assert!(!content.contains("another-secret"), "leaked: {content}");
        assert!(content.contains("[REDACTED]"), "{content}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn disabled_log_writes_nothing_and_does_not_create_the_file() {
        let dir = temp_dir();
        let path = dir.join("launcher.log");
        let log = LauncherLog::default();
        log.write(Level::Info, "写不出去的日志");
        assert!(!log.is_enabled());
        assert!(!path.exists(), "a disabled log must not create a file");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn appended_across_sessions_instead_of_truncated() {
        let dir = temp_dir();
        let path = dir.join("launcher.log");
        for message in ["第一次会话", "第二次会话"] {
            let log = LauncherLog::default();
            log.enable(&path).unwrap();
            log.write(Level::Info, message);
            log.disable();
        }
        let content = std::fs::read_to_string(&path).unwrap();
        // 覆盖会让「重启一次」丢掉上一次的证据。
        assert!(content.contains("第一次会话"), "{content}");
        assert!(content.contains("第二次会话"), "{content}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn oversized_file_rotates_and_keeps_one_backup() {
        let dir = temp_dir();
        let path = dir.join("launcher.log");
        // 先塞一个超过上限的文件，模拟长期使用后的状态。
        std::fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        let log = LauncherLog::default();
        log.enable(&path).unwrap();
        log.write(Level::Info, "轮转之后的第一行");
        log.disable();

        assert!(
            rotated_path(&path, 1).exists(),
            "a rotated copy must be kept"
        );
        assert!(rotated_path(&path, 2).is_empty() || !rotated_path(&path, 2).exists());
        let current = std::fs::read_to_string(&path).unwrap();
        assert!(current.contains("轮转之后的第一行"), "{current}");
        // 轮转后当前文件应当很小，而不是继续增长。
        assert!(current.len() < MAX_BYTES as usize, "{}", current.len());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 写失败后必须自己关掉，否则每一行都会再撞一次失败的系统调用。
    ///
    /// ## 怎么造一个必然失败的写入
    ///
    /// 第一版是「删掉文件所在目录」——在 Unix 上这**不会**让写入失败：已打开的
    /// 文件描述符继续有效，数据写进一个已经没有名字的 inode。那个测试因此永远
    /// 通过不了，也说明了它测的不是我以为的东西。
    ///
    /// 真正可靠的造法是让描述符本身不可写：用只读方式打开文件再塞进 writer。
    /// 这正是「磁盘满 / 只读挂载」在系统调用层面的样子。
    #[test]
    fn a_write_failure_disables_the_log_instead_of_looping() {
        let dir = temp_dir();
        let path = dir.join("launcher.log");
        std::fs::write(&path, b"existing\n").unwrap();
        let log = LauncherLog::default();
        // 手工装一个只读句柄：`enable` 打开的总是可写的，这里绕开它。
        let readonly = std::fs::File::open(&path).unwrap();
        {
            let mut guard = log.inner.lock().unwrap();
            *guard = Some(Writer {
                file: readonly,
                path: path.clone(),
                written: 0,
            });
        }
        log.write(Level::Info, "这次写入会失败");
        assert!(!log.is_enabled(), "a failing log must disable itself");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 记录实例名与路径是可以接受的，但日志文件本身必须是私有的。
    ///
    /// 内容里有实例名、模块名、文件路径——这些对报 issue 是必要的（"装到哪个目录"、
    /// "哪个模组"），对同机的其他用户则不必要。所以文件权限收到 0o600：
    /// 内容层面的取舍改变不了「谁能读这个文件」这件事。
    #[cfg(unix)]
    #[test]
    fn log_file_is_private_to_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir();
        let path = dir.join("launcher.log");
        let log = LauncherLog::default();
        log.enable(&path).unwrap();
        log.write(Level::Info, "实例名与路径会出现在这里");
        log.disable();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "log file mode is {mode:o}, expected 600");
        let _ = std::fs::remove_dir_all(dir);
    }
}
