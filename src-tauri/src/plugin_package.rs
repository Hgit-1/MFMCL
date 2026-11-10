//! Plugin package installation, removal, and the on-disk layout.
//!
//! ## What a package is
//!
//! A plugin package is a ZIP archive (`.zip`, `.mflp`, `.mfplugin` — the extension
//! is a hint, not a contract) with a manifest at the root:
//!
//! ```text
//! manifest.json          required
//! main.js                optional, the script entry point
//! assets/…               optional, referenced by relative path from the manifest
//! ```
//!
//! The manifest is the *same* shape the paste-a-manifest flow already accepts
//! (`utils/plugins.ts::normalizePlugin`), plus the fields a package needs:
//! `main`, `files`, and an optional `sha256` self-check. Keeping one shape means
//! a user who pastes a manifest and a user who imports a package end up with the
//! same record, and the store does not grow two nearly-identical plugin types.
//!
//! ## Why installation is a Rust concern and not a frontend one
//!
//! Three things cannot be done safely in the WebView:
//!
//!   1. **Reading a file the user picked**, and reading it as bytes rather than
//!      trusting a `<input type="file">` to hand over a whole archive.
//!   2. **Validating entry paths.** ZIP entries are attacker-controlled strings;
//!      `../../.bashrc` in a central directory is the classic zip-slip. The
//!      WebView has no filesystem authority to enforce that against, and doing it
//!      only in JS would mean the next frontend re-implements it.
//!   3. **Writing to the plugin directory** with a bounded size, an atomic swap,
//!      and a documented location the user can inspect.
//!
//! ## Threat model — stated plainly, because the frontend cannot enforce it
//!
//! Packages are untrusted input. This module defends against the archive-level
//! attacks: zip slip, absolute paths, Windows reserved names, control characters,
//! decompression bombs (entry count, per-entry size, total size, compression
//! ratio), duplicate entries, symlink-esque mode bits, and oversized manifests.
//!
//! It does **not** defend against a malicious plugin script. A `main.js` runs in
//! the WebView, and a WebView in a Tauri app can reach the IPC bridge. The Worker
//! sandbox in `frontend/pluginSandbox.ts` removes the obvious primitives, but a
//! determined script can still exhaust CPU or memory. That is why the UI states
//! "third-party plugin, not sandboxed" instead of claiming safety, and why the
//! default for a newly imported plugin is *enabled but inert* until the user
//! confirms in the install dialog.

use launcher_core::modpacks::safe_pack_path;
use std::collections::HashSet;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

/// Hard ceiling on the whole extracted payload.
///
/// 32 MiB is chosen from the largest thing a plugin legitimately ships: the Aria
/// artwork set is eight WebP files at ~250 KiB each, about 2 MiB. Two orders of
/// magnitude of headroom still leaves a package far too small to be a useful
/// decompression bomb.
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;

/// Hard ceiling on one extracted file. An icon or a script, not a video.
const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;

/// Hard ceiling on entry count. A plugin is a handful of files; a thousand
/// entries means either a mistake or an attempt to make extraction expensive.
const MAX_ENTRIES: usize = 256;

/// Ceiling on the manifest itself. It is JSON describing a plugin, and a large
/// one is a sign of something other than a manifest.
const MAX_MANIFEST_BYTES: u64 = 512 * 1024;

/// Ceiling on the compressed archive as received.
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;

/// A compression ratio above this is treated as a bomb.
///
/// 200:1 is far above any real archive of text and images (WebP and PNG are
/// already compressed, so a package full of them sits near 1:1). A hit here is
/// not proof of malice, but it is proof that extracting without asking is a bad
/// idea.
const MAX_COMPRESSION_RATIO: u64 = 200;

/// Where one installed plugin lives, relative to the plugin root.
#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    pub id: String,
    pub version: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Manifest(String),
}

type Result<T> = std::result::Result<T, PackageError>;

fn rejected(reason: impl Into<String>) -> PackageError {
    PackageError::Rejected(reason.into())
}

fn io_error(context: &str, error: std::io::Error) -> PackageError {
    PackageError::Io(format!("{context}：{error}"))
}

// ---------------------------------------------------------------------------
// Manifest validation
// ---------------------------------------------------------------------------

/// The parts of a manifest this module needs to act on.
///
/// The full manifest is validated by the frontend (`normalizePlugin`), which owns
/// the display fields and the i18n dictionary keys. Duplicating that validation
/// here would create two sources of truth that drift. What the backend must know
/// is only: which id to store it under, which version, and which file to run.
#[derive(Debug, Clone)]
pub struct ManifestSummary {
    pub id: String,
    pub version: String,
    pub main: Option<String>,
}

/// Parse the manifest out of the archive and validate the security-relevant bits.
///
/// Deliberately tolerant about everything except identifiers and paths: an
/// unknown field is preserved for the frontend, a bad `id` is rejected here
/// because it becomes a directory name.
pub fn parse_manifest(bytes: &[u8]) -> Result<ManifestSummary> {
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(rejected("插件清单过大。"));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| PackageError::Manifest(format!("插件清单不是合法 JSON：{error}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| PackageError::Manifest("插件清单必须是一个对象。".into()))?;

    let id = object
        .get("id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or_default();
    // Same rule as the frontend's `normalizePlugin`, and for the same reason:
    // the id becomes a directory name and a storage key. Allowing `.` would let
    // `a..b` through, so the charset is explicit rather than "not a slash".
    if id.is_empty() || id.len() > 64 {
        return Err(rejected("插件 id 缺失或过长。"));
    }
    if !id
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(rejected(
            "插件 id 只能包含字母、数字、点、下划线和连字符，且必须以字母或数字开头。",
        ));
    }

    let version = object
        .get("version")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or_default();
    if version.is_empty() || version.len() > 24 {
        return Err(rejected("插件版本号缺失或过长。"));
    }

    // `main` is optional: a manifest-only plugin (like Aria, which contributes
    // artwork rather than code) has no script. But when present, it must stay
    // inside the package.
    let main = match object.get("main").and_then(|value| value.as_str()) {
        Some(raw) => {
            let path =
                safe_pack_path(raw.trim()).map_err(|_| rejected("插件入口文件路径不安全。"))?;
            Some(path.to_string_lossy().replace('\\', "/"))
        }
        None => None,
    };

    Ok(ManifestSummary {
        id: id.to_owned(),
        version: version.to_owned(),
        main,
    })
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

/// One file to write: a validated relative path plus its bytes.
#[derive(Debug)]
struct ExtractedFile {
    path: PathBuf,
    bytes: Vec<u8>,
}

/// Read the archive into memory, validating every entry before anything is
/// written.
///
/// ## Why fully in memory before writing
///
/// The alternative — stream each entry to disk as it is read — means a package
/// that fails on entry 200 has already written 199 files. Those files would then
/// need cleaning up, and a crash between the two leaves a half-installed plugin
/// that the scanner will happily load. Validating the whole archive first makes
/// the failure mode "nothing happened", which is the only one worth having for
/// an install operation.
fn read_archive(archive: &[u8]) -> Result<(ManifestSummary, Vec<ExtractedFile>)> {
    if archive.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(rejected("插件包过大。"));
    }

    /*
     * ## Duplicate entries have to be counted before `zip` sees them
     *
     * `zip::ZipArchive` silently de-duplicates: given an archive whose central
     * directory lists `a.txt` twice, `zip.len()` reports one fewer entry and
     * `by_index` never yields the second copy. That makes the per-entry
     * `seen` check below useless against the attack it was written for, and the
     * attack is the interesting one — two entries with the same name mean "which
     * file ends up on disk" depends on extraction order, which is exactly what an
     * attacker controls and a user cannot see in their file manager.
     *
     * This was caught by a test that failed for weeks: an earlier version of the
     * test built its archive with `ZipWriter`, which refuses duplicate names, so
     * the archive never actually contained two copies. The check was unreachable
     * and everyone assumed it worked.
     *
     * So the central directory is scanned here, before the archive is handed to
     * `zip`, and any name that appears twice is refused outright. Doing it on raw
     * bytes costs one linear pass and cannot be bypassed by a parser quirk.
     */
    let raw_names = central_directory_names(archive)?;
    if raw_names.len() > MAX_ENTRIES {
        return Err(rejected(format!(
            "压缩包条目过多（{} 个，上限 {MAX_ENTRIES}）。",
            raw_names.len()
        )));
    }
    {
        let mut raw_seen: HashSet<&str> = HashSet::new();
        for name in &raw_names {
            if !raw_seen.insert(name.as_str()) {
                return Err(rejected(format!("压缩包包含重复条目：{name}")));
            }
        }
    }

    let mut zip = zip::ZipArchive::new(Cursor::new(archive))
        .map_err(|error| rejected(format!("无法读取压缩包：{error}")))?;

    if zip.len() > MAX_ENTRIES {
        return Err(rejected(format!(
            "压缩包条目过多（{} 个，上限 {MAX_ENTRIES}）。",
            zip.len()
        )));
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut files: Vec<ExtractedFile> = Vec::new();
    let mut total: u64 = 0;
    let mut manifest_bytes: Option<Vec<u8>> = None;

    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| rejected(format!("压缩包条目损坏：{error}")))?;

        // Directories carry no data; their names still need no validation
        // because nothing is created for them (parent dirs are made on demand),
        // but a directory entry is how zip-slip often starts, so it is checked
        // anyway and then ignored.
        let raw_name = entry
            .name()
            .map_err(|error| rejected(format!("压缩包条目名称无法解码：{error}")))?
            .to_owned();
        if raw_name.ends_with('/') {
            if safe_pack_path(raw_name.trim_end_matches('/')).is_err() {
                return Err(rejected("压缩包包含不安全的目录路径。"));
            }
            continue;
        }

        let relative = safe_pack_path(&raw_name)
            .map_err(|_| rejected(format!("压缩包包含不安全的文件路径：{raw_name}")))?;

        // Duplicate names would make "which one wins" depend on extraction
        // order, which is attacker-controllable and not what the user sees in
        // the archive listing.
        let key = relative.to_string_lossy().replace('\\', "/");
        if !seen.insert(key.clone()) {
            return Err(rejected(format!("压缩包包含重复条目：{key}")));
        }

        // Symlinks in a ZIP are encoded in the external attributes. Refusing them
        // outright is simpler than reasoning about what a link could point at on
        // three platforms.
        #[cfg(unix)]
        {
            if entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                return Err(rejected(format!("压缩包包含符号链接：{key}")));
            }
        }

        let declared = entry.size();
        if declared > MAX_ENTRY_BYTES {
            return Err(rejected(format!(
                "压缩包条目过大：{key}（{} 字节，上限 {MAX_ENTRY_BYTES}）",
                declared
            )));
        }
        let compressed = entry.compressed_size();
        if compressed > 0 && declared / compressed.max(1) > MAX_COMPRESSION_RATIO {
            return Err(rejected(format!(
                "压缩包条目压缩比异常：{key}（可能是压缩炸弹）"
            )));
        }

        // Read with the declared size as a budget, not as a promise: a lying
        // header is exactly what a bomb looks like, so the reader is capped too.
        let limit = MAX_ENTRY_BYTES.min(declared.saturating_add(1));
        let mut buffer = Vec::new();
        let read = entry
            .by_ref()
            .take(limit)
            .read_to_end(&mut buffer)
            .map_err(|error| rejected(format!("读取压缩包条目失败：{key}：{error}")))?;
        if read as u64 > MAX_ENTRY_BYTES {
            return Err(rejected(format!("压缩包条目解压后过大：{key}")));
        }

        total += buffer.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(rejected(format!(
                "插件包解压后过大（上限 {} MiB）。",
                MAX_TOTAL_BYTES / 1024 / 1024
            )));
        }

        // The manifest may be at the root or in a single top-level directory,
        // which is what GitHub's "Download ZIP" produces. Accepting the wrapped
        // form saves every user from having to unpack and repack.
        let name = relative
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.eq_ignore_ascii_case("manifest.json") && key.matches('/').count() <= 1 {
            manifest_bytes = Some(buffer.clone());
        }

        files.push(ExtractedFile {
            path: relative,
            bytes: buffer,
        });
    }

    let manifest_bytes = manifest_bytes.ok_or_else(|| rejected("插件包里没有 manifest.json。"))?;
    let manifest = parse_manifest(&manifest_bytes)?;

    // If the manifest was nested (GitHub's `repo-main/manifest.json`), strip that
    // prefix from every file so the installed layout is flat. Without this the
    // plugin's `main` path would resolve relative to the wrong place.
    let prefix = files
        .iter()
        .find(|file| {
            file.path
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("manifest.json"))
        })
        .and_then(|file| file.path.parent().map(Path::to_path_buf))
        .filter(|parent| !parent.as_os_str().is_empty());

    let files = match prefix {
        Some(prefix) => files
            .into_iter()
            .filter_map(|file| {
                file.path
                    .strip_prefix(&prefix)
                    .ok()
                    .map(|stripped| ExtractedFile {
                        path: stripped.to_path_buf(),
                        bytes: file.bytes,
                    })
            })
            .filter(|file| !file.path.as_os_str().is_empty())
            .collect(),
        None => files,
    };

    // The entry point must actually be in the package. A manifest that names a
    // file which is not there produces a plugin that fails at load time with a
    // message the user cannot act on.
    if let Some(main) = &manifest.main {
        let present = files
            .iter()
            .any(|file| file.path.to_string_lossy().replace('\\', "/") == *main);
        if !present {
            return Err(rejected(format!("插件清单声明的入口文件不在包内：{main}")));
        }
    }

    Ok((manifest, files))
}

// ---------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------

/// Install a package into `root/<id>/`, replacing any previous version.
///
/// ## Why the swap is staged
///
/// Extraction writes to `root/.staging-<uuid>/`, and only when every file is on
/// disk does the old directory get moved aside and the new one renamed into
/// place. A failure at any point leaves the previous installation intact, which
/// matters because "my plugin stopped working after an update" is a much worse
/// outcome than "the update did not apply".
/// Every entry name in the archive's central directory, in file order.
///
/// ## Why parse by hand instead of asking `zip`
///
/// `zip::ZipArchive` de-duplicates names: an archive listing `a.txt` twice comes
/// back with one `a.txt`. That is a defensible choice for a reader, and it is
/// fatal for a validator — the duplicate is exactly what needs to be *detected*,
/// and it is invisible through that API. See the note in `read_archive`.
///
/// The parse is deliberately minimal: walk the central directory signature
/// (`PK\x01\x02`), read the 64-bit-safe name length and the name, skip the rest.
/// It does not need to understand compression, because it never decompresses
/// anything. Anything malformed is refused rather than skipped — a directory the
/// parser cannot follow is a directory whose contents cannot be validated.
fn central_directory_names(archive: &[u8]) -> Result<Vec<String>> {
    const EOCD_SIGNATURE: u32 = 0x0605_4b50;
    const EOCD_MIN_BYTES: usize = 22;
    const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
    /// Fixed part of a central directory record, up to the file name.
    const CENTRAL_HEADER_BYTES: usize = 46;

    if archive.len() < EOCD_MIN_BYTES {
        return Err(rejected("压缩包过小，缺少目录结构。"));
    }

    // The end-of-central-directory record is at the tail, possibly followed by a
    // comment of up to 65535 bytes. Walk backwards to find its signature.
    let tail_start = archive.len().saturating_sub(EOCD_MIN_BYTES + 65_535);
    let mut eocd = None;
    for position in (tail_start..=archive.len() - EOCD_MIN_BYTES).rev() {
        if archive[position..position + 4] == EOCD_SIGNATURE.to_le_bytes() {
            eocd = Some(position);
            break;
        }
    }
    let eocd = eocd.ok_or_else(|| rejected("压缩包缺少中央目录。"))?;

    let read_u16 = |offset: usize| -> Result<usize> {
        archive
            .get(offset..offset + 2)
            .map(|slice| u16::from_le_bytes([slice[0], slice[1]]) as usize)
            .ok_or_else(|| rejected("压缩包中央目录不完整。"))
    };
    let read_u32 = |offset: usize| -> Result<usize> {
        archive
            .get(offset..offset + 4)
            .map(|slice| u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]) as usize)
            .ok_or_else(|| rejected("压缩包中央目录不完整。"))
    };

    let declared_count = read_u16(eocd + 10)?;
    let directory_offset = read_u32(eocd + 16)?;
    if directory_offset >= archive.len() {
        return Err(rejected("压缩包中央目录偏移越界。"));
    }

    let mut names = Vec::with_capacity(declared_count.min(MAX_ENTRIES));
    let mut cursor = directory_offset;
    // Bound the loop by the declared count *and* by the file size: a hostile
    // archive can claim 65535 entries in a 200-byte file, and trusting the count
    // alone turns that into an infinite loop reading past the end.
    for _ in 0..declared_count {
        if cursor + CENTRAL_HEADER_BYTES > archive.len() {
            return Err(rejected("压缩包中央目录在条目中途结束。"));
        }
        if archive[cursor..cursor + 4] != CENTRAL_SIGNATURE.to_le_bytes() {
            // A gap here means either a zip64 record we do not support or a
            // malformed directory. Both are refused rather than guessed at.
            return Err(rejected("压缩包中央目录格式不受支持。"));
        }
        let name_len = read_u16(cursor + 28)?;
        let extra_len = read_u16(cursor + 30)?;
        let comment_len = read_u16(cursor + 32)?;
        let name_start = cursor + CENTRAL_HEADER_BYTES;
        let name_end = name_start
            .checked_add(name_len)
            .filter(|end| *end <= archive.len())
            .ok_or_else(|| rejected("压缩包条目名越界。"))?;
        let name = String::from_utf8_lossy(&archive[name_start..name_end]).into_owned();
        names.push(name);
        cursor = name_end
            .checked_add(extra_len)
            .and_then(|offset| offset.checked_add(comment_len))
            .ok_or_else(|| rejected("压缩包中央目录偏移溢出。"))?;
    }

    Ok(names)
}

pub fn install_package(root: &Path, archive: &[u8]) -> Result<InstalledPlugin> {
    let (manifest, files) = read_archive(archive)?;

    std::fs::create_dir_all(root).map_err(|error| io_error("无法创建插件目录", error))?;

    let staging = root.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<PathBuf> {
        std::fs::create_dir_all(&staging).map_err(|error| io_error("无法创建暂存目录", error))?;
        for file in &files {
            let target = staging.join(&file.path);
            // The path was validated relative to the package root, and `join`
            // with a relative path cannot escape `staging` — but the parent
            // still has to exist.
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| io_error("无法创建插件子目录", error))?;
            }
            let mut handle = std::fs::File::create(&target)
                .map_err(|error| io_error("无法写入插件文件", error))?;
            handle
                .write_all(&file.bytes)
                .map_err(|error| io_error("无法写入插件文件", error))?;
        }
        Ok(staging.clone())
    })();

    let staging = match result {
        Ok(path) => path,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };

    let target = root.join(&manifest.id);
    // Refuse a target that exists as a file: renaming a directory onto a file
    // fails with a confusing OS error, and a file in that position means the
    // plugin root has been tampered with.
    if target.is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(rejected(format!(
            "插件目录位置已被文件占用：{}",
            target.display()
        )));
    }

    let backup = root.join(format!(".previous-{}", uuid::Uuid::new_v4()));
    let had_previous = target.is_dir();
    if had_previous {
        std::fs::rename(&target, &backup).map_err(|error| {
            let _ = std::fs::remove_dir_all(&staging);
            io_error("无法移走旧版本插件", error)
        })?;
    }

    if let Err(error) = std::fs::rename(&staging, &target) {
        // Put the old version back. If this fails too, the plugin is gone — but
        // reporting the original error is still the most useful thing to do.
        if had_previous {
            let _ = std::fs::rename(&backup, &target);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(io_error("无法启用新版本插件", error));
    }

    if had_previous {
        let _ = std::fs::remove_dir_all(&backup);
    }

    Ok(InstalledPlugin {
        id: manifest.id,
        version: manifest.version,
    })
}

/// Remove one installed plugin.
///
/// Only directories directly under `root` are removable, and the id is
/// re-validated: this function receives an id from the frontend, and a
/// compromised frontend must not be able to turn "remove plugin" into
/// "remove anything".
pub fn remove_plugin(root: &Path, id: &str) -> Result<()> {
    validate_id(id)?;
    let target = root.join(id);
    if !target.is_dir() {
        return Err(rejected(format!("没有已安装的插件：{id}")));
    }
    // Defence in depth: confirm the resolved path is still inside the root.
    // A symlink planted at `root/<id>` would otherwise let this delete elsewhere.
    let root_real =
        std::fs::canonicalize(root).map_err(|error| io_error("无法解析插件目录", error))?;
    let target_real =
        std::fs::canonicalize(&target).map_err(|error| io_error("无法解析插件目录", error))?;
    if !target_real.starts_with(&root_real) || target_real == root_real {
        return Err(rejected("拒绝删除插件目录之外的路径。"));
    }
    std::fs::remove_dir_all(&target_real).map_err(|error| io_error("无法删除插件目录", error))
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        || id.contains("..")
    {
        return Err(rejected("插件 id 不合法。"));
    }
    Ok(())
}

/// Read one installed plugin's manifest back out.
///
/// Used by the scanner on startup, and by `plugin_reload` so the frontend can
/// refresh a single plugin's record without re-reading every package.
pub fn read_installed(root: &Path, id: &str) -> Result<(serde_json::Value, PathBuf)> {
    validate_id(id)?;
    let directory = root.join(id);
    let manifest_path = directory.join("manifest.json");
    let bytes =
        std::fs::read(&manifest_path).map_err(|error| io_error("无法读取已安装插件清单", error))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(rejected("已安装插件清单异常过大。"));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| PackageError::Manifest(format!("已安装插件清单损坏：{error}")))?;
    Ok((value, directory))
}

/// Validate one source-index entry.
///
/// ## Why this lives here rather than in the command layer
///
/// The rules are the same ones the package installer applies, and they exist for
/// the same reason: an entry's `download` URL is fetched and its bytes are
/// extracted, so a `http://` or `file://` URL in an index is a way to turn "click
/// install" into "read local files" or "fetch over a downgraded transport".
///
/// Checking at index time rather than only at install time means the user sees a
/// broken entry in the store listing instead of discovering it after clicking
/// install. It is advisory — `fetch_package` re-checks — but it keeps the list
/// honest.
pub fn validate_entry(
    entry: &crate::plugin_install::IndexEntry,
) -> std::result::Result<(), String> {
    if entry.id.is_empty() || entry.id.len() > 64 {
        return Err("id 缺失或过长".into());
    }
    if !entry
        .id
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        || !entry
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("id 含有不允许的字符".into());
    }
    if entry.version.trim().is_empty() || entry.version.len() > 24 {
        return Err("版本号缺失或过长".into());
    }
    let parsed =
        reqwest::Url::parse(&entry.download).map_err(|_| "下载地址格式无效".to_string())?;
    if parsed.scheme() != "https" {
        return Err("下载地址必须使用 HTTPS".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("下载地址不能包含账号信息".into());
    }
    if let Some(homepage) = &entry.homepage {
        let parsed = reqwest::Url::parse(homepage).map_err(|_| "主页地址格式无效".to_string())?;
        if !matches!(parsed.scheme(), "https" | "http") {
            return Err("主页地址必须是 HTTP(S)".into());
        }
    }
    /*
     * The optional hash pin. Checked here so a malformed pin is reported at index
     * time, where the user is browsing, rather than at install time after a
     * multi-megabyte download.
     */
    if let Some(hash) = &entry.sha256 {
        let trimmed = hash.trim();
        if !trimmed.is_empty()
            && (trimmed.len() != 64 || !trimmed.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return Err("sha256 必须是 64 位十六进制".into());
        }
    }
    Ok(())
}

/// Every installed plugin, sorted by id.
///
/// A directory without a readable manifest is skipped rather than reported: it is
/// either a leftover staging directory (the `.staging-` prefix) or something the
/// user dropped in by hand, and neither is a reason to fail the whole list.
pub fn scan_installed(root: &Path) -> Vec<InstalledPlugin> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut plugins = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if let Ok((value, _directory)) = read_installed(root, &name) {
            let version = value
                .get("version")
                .and_then(|value| value.as_str())
                .unwrap_or("0.0.0")
                .to_owned();
            plugins.push(InstalledPlugin { id: name, version });
        }
    }
    plugins.sort_by(|left, right| left.id.cmp(&right.id));
    plugins
}

// ---------------------------------------------------------------------------
// Fetching a package over the network
// ---------------------------------------------------------------------------

/// Fetch a package from an HTTPS URL.
///
/// ## Why HTTPS is enforced by scheme rather than by redirect
///
/// The frontend can be talked into requesting `http://` by a malicious source
/// index, and a package fetched over plain HTTP can be swapped in transit —
/// which turns "install this plugin" into "install whatever the network handed
/// you". The scheme check happens here, after redirects have been followed, so a
/// `https` URL that redirects to `http` is rejected too.
///
/// The response is bounded while streaming, not after: a `Content-Length` header
/// is a claim, and trusting it means a malicious server can make the launcher
/// buffer an unbounded body before noticing.
pub async fn fetch_package(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let parsed = reqwest::Url::parse(url).map_err(|_| rejected("插件下载地址格式无效。"))?;
    if parsed.scheme() != "https" {
        return Err(rejected("插件只能从 HTTPS 地址安装。"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(rejected("插件下载地址不能包含账号信息。"));
    }

    let response = client
        .get(parsed)
        .send()
        .await
        .map_err(|error| PackageError::Io(format!("下载插件包失败：{error}")))?;
    let response = response
        .error_for_status()
        .map_err(|error| PackageError::Io(format!("下载插件包失败：{error}")))?;

    // After redirects. A server that bounces HTTPS to HTTP has downgraded the
    // transport, and the bytes cannot be trusted regardless of what the first
    // response said.
    if response.url().scheme() != "https" {
        return Err(rejected("插件下载被重定向到非 HTTPS 地址，已拒绝。"));
    }

    if let Some(length) = response.content_length()
        && length > MAX_ARCHIVE_BYTES
    {
        return Err(rejected("插件包过大。"));
    }

    let mut buffer: Vec<u8> = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| PackageError::Io(format!("下载插件包中断：{error}")))?
    {
        if buffer.len() as u64 + chunk.len() as u64 > MAX_ARCHIVE_BYTES {
            return Err(rejected("插件包过大。"));
        }
        buffer.extend_from_slice(&chunk);
    }
    Ok(buffer)
}

/// Verify a package against the SHA-256 the index pinned.
///
/// ## Why this is separate from `fetch_package`
///
/// `fetch_package` is also used for a URL the user typed, where there is nothing
/// to compare against. Keeping the check separate means the caller decides
/// whether a pin exists, and the absence of a pin is never mistaken for a pass.
///
/// ## Why it compares lowercase hex explicitly
///
/// An index maintained by hand will eventually contain an uppercase hash. Failing
/// that install would be technically correct and practically maddening, so the
/// comparison is case-insensitive. What it does *not* do is accept a hash of the
/// wrong length: a truncated pin would match far too easily.
pub fn verify_package_sha256(bytes: &[u8], expected: &str) -> Result<()> {
    use sha2::{Digest, Sha256};

    let expected = expected.trim();
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(rejected(
            "插件源给出的 SHA-256 格式无效（应为 64 位十六进制）。",
        ));
    }
    let actual = hex::encode(Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(PackageError::Manifest(format!(
            "插件包校验失败：期望 {expected}，实际 {actual}。文件可能已被替换或下载不完整。"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn package(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = SimpleFileOptions::default();
            for (name, bytes) in entries {
                /*
                 * `start_file` refuses a name it has already written, so a naive
                 * `package()` cannot construct the duplicate-entry archive this
                 * test suite needs to prove is rejected. Swallow that error: the
                 * duplicate test builds its archive by hand below.
                 */
                if writer.start_file(*name, options).is_err() {
                    continue;
                }
                let _ = writer.write_all(bytes);
            }
            let _ = writer.finish();
        }
        buffer.into_inner()
    }

    /// Build an archive that really contains two entries with the same name.
    ///
    /// `zip::ZipWriter` prevents this on purpose, which is good, but it means the
    /// duplicate-entry defence cannot be tested through the normal writer. Writing
    /// the local file headers and the central directory by hand is the only way to
    /// produce the exact bytes a hostile archive would contain.
    fn package_with_duplicate(
        name: &str,
        first: &[u8],
        second: &[u8],
        extra: &[(&str, &[u8])],
    ) -> Vec<u8> {
        fn crc32(data: &[u8]) -> u32 {
            let mut crc = 0xffff_ffffu32;
            for byte in data {
                crc ^= *byte as u32;
                for _ in 0..8 {
                    let mask = (crc & 1).wrapping_neg();
                    crc = (crc >> 1) ^ (0xedb8_8320 & mask);
                }
            }
            !crc
        }

        fn local_header(name: &str, data: &[u8]) -> Vec<u8> {
            let name_bytes = name.as_bytes();
            let mut header = Vec::new();
            header.extend_from_slice(&0x04034b50u32.to_le_bytes());
            header.extend_from_slice(&20u16.to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes()); // stored, not deflated
            header.extend_from_slice(&0u16.to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes());
            header.extend_from_slice(&crc32(data).to_le_bytes());
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes());
            header.extend_from_slice(name_bytes);
            header.extend_from_slice(data);
            header
        }

        fn central_header(name: &str, data: &[u8], offset: u32) -> Vec<u8> {
            let name_bytes = name.as_bytes();
            let mut entry = Vec::new();
            entry.extend_from_slice(&0x02014b50u32.to_le_bytes());
            entry.extend_from_slice(&20u16.to_le_bytes()); // version made by
            entry.extend_from_slice(&20u16.to_le_bytes()); // version needed
            entry.extend_from_slice(&0u16.to_le_bytes()); // flags
            entry.extend_from_slice(&0u16.to_le_bytes()); // method
            entry.extend_from_slice(&0u16.to_le_bytes()); // time
            entry.extend_from_slice(&0u16.to_le_bytes()); // date
            entry.extend_from_slice(&crc32(data).to_le_bytes());
            entry.extend_from_slice(&(data.len() as u32).to_le_bytes());
            entry.extend_from_slice(&(data.len() as u32).to_le_bytes());
            entry.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            entry.extend_from_slice(&0u16.to_le_bytes()); // extra
            entry.extend_from_slice(&0u16.to_le_bytes()); // comment
            entry.extend_from_slice(&0u16.to_le_bytes()); // disk
            entry.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
            entry.extend_from_slice(&0u32.to_le_bytes()); // external attributes
            entry.extend_from_slice(&offset.to_le_bytes());
            entry.extend_from_slice(name_bytes);
            entry
        }

        // A deliberately hostile ordering: the duplicate is separated from its
        // twin by the manifest, which is what makes "last one wins" and "first
        // one wins" produce different plugins.
        let ordered: Vec<(&str, &[u8])> = {
            let mut items = vec![(name, first)];
            items.extend(extra.iter().copied());
            items.push((name, second));
            items
        };

        let mut body: Vec<u8> = Vec::new();
        let mut directory: Vec<u8> = Vec::new();
        for (entry_name, data) in &ordered {
            let offset = body.len() as u32;
            body.extend_from_slice(&local_header(entry_name, data));
            directory.extend_from_slice(&central_header(entry_name, data, offset));
        }

        let directory_offset = body.len() as u32;
        let directory_len = directory.len() as u32;
        body.extend_from_slice(&directory);
        body.extend_from_slice(&0x06054b50u32.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&(ordered.len() as u16).to_le_bytes());
        body.extend_from_slice(&(ordered.len() as u16).to_le_bytes());
        body.extend_from_slice(&directory_len.to_le_bytes());
        body.extend_from_slice(&directory_offset.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body
    }

    const MANIFEST: &[u8] = br#"{
        "id": "probe",
        "version": "1.0.0",
        "name": "Probe",
        "description": "test"
    }"#;

    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!("mfmcl-plugin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_minimal_package_installs_and_is_scanned_back() {
        let root = root();
        let bytes = package(&[("manifest.json", MANIFEST)]);
        let installed = install_package(&root, &bytes).unwrap();
        assert_eq!(installed.id, "probe");
        assert_eq!(installed.version, "1.0.0");
        assert!(root.join("probe").join("manifest.json").is_file());

        let scanned = scan_installed(&root);
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].id, "probe");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_github_wrapped_package_is_flattened() {
        // GitHub's "Download ZIP" wraps everything in `repo-branch/`. Installing
        // that must not produce a plugin whose files live one level too deep.
        let root = root();
        let bytes = package(&[
            ("repo-main/manifest.json", MANIFEST),
            ("repo-main/main.js", b"export {}"),
        ]);
        install_package(&root, &bytes).unwrap();
        assert!(root.join("probe").join("manifest.json").is_file());
        assert!(root.join("probe").join("main.js").is_file());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn zip_slip_paths_are_refused() {
        let root = root();
        for evil in ["../escape.txt", "/etc/passwd", "a/../../b.txt"] {
            let bytes = package(&[("manifest.json", MANIFEST), (evil, b"x")]);
            let error = install_package(&root, &bytes).unwrap_err();
            assert!(
                matches!(error, PackageError::Rejected(_)),
                "{evil} must be rejected, got {error}"
            );
        }
        // Nothing was written outside, and nothing was installed.
        assert!(scan_installed(&root).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_windows_reserved_name_is_refused() {
        // `CON` and friends are unusable on Windows and a legitimate package never
        // contains them, so they are a reliable signal of a malformed archive.
        let root = root();
        let bytes = package(&[("manifest.json", MANIFEST), ("assets/CON", b"x")]);
        assert!(install_package(&root, &bytes).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_manifest_without_id_is_refused() {
        let error = parse_manifest(br#"{"version":"1.0.0"}"#).unwrap_err();
        assert!(matches!(error, PackageError::Rejected(_)));
    }

    #[test]
    fn an_id_that_could_escape_the_plugin_root_is_refused() {
        for id in ["../evil", "a/b", "..", "-leading", ".hidden"] {
            let document = format!(r#"{{"id":"{id}","version":"1"}}"#);
            assert!(
                parse_manifest(document.as_bytes()).is_err(),
                "{id} must be rejected as an id"
            );
        }
    }

    #[test]
    fn a_missing_entry_point_is_refused() {
        let manifest = br#"{"id":"probe","version":"1","main":"main.js"}"#;
        let bytes = package(&[("manifest.json", manifest)]);
        let error = read_archive(&bytes).unwrap_err();
        assert!(
            matches!(error, PackageError::Rejected(_)),
            "a declared entry point must exist, got {error}"
        );
    }

    #[test]
    fn an_entry_point_that_escapes_is_refused() {
        let manifest = br#"{"id":"probe","version":"1","main":"../../run.sh"}"#;
        assert!(parse_manifest(manifest).is_err());
    }

    #[test]
    fn a_package_without_a_manifest_is_refused() {
        let root = root();
        let bytes = package(&[("main.js", b"export {}")]);
        assert!(install_package(&root, &bytes).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_entries_are_refused() {
        let root = root();
        let bytes = package_with_duplicate("a.txt", b"1", b"2", &[("manifest.json", MANIFEST)]);
        let error = install_package(&root, &bytes).unwrap_err();
        assert!(matches!(error, PackageError::Rejected(_)), "got {error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn updating_keeps_the_previous_version_when_the_new_one_is_bad() {
        // The point of the staging swap: a failed update must not destroy a
        // working install.
        let root = root();
        let good = br#"{"id":"probe","version":"1.0.0"}"#;
        install_package(&root, &package(&[("manifest.json", good)])).unwrap();

        let bad = package(&[("manifest.json", br#"{"id":"probe"}"#)]);
        assert!(install_package(&root, &bad).is_err());

        let scanned = scan_installed(&root);
        assert_eq!(scanned.len(), 1);
        assert_eq!(
            scanned[0].version, "1.0.0",
            "the working version must survive"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn removing_a_plugin_cleans_the_directory() {
        let root = root();
        install_package(&root, &package(&[("manifest.json", MANIFEST)])).unwrap();
        remove_plugin(&root, "probe").unwrap();
        assert!(!root.join("probe").exists());
        assert!(scan_installed(&root).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn removing_refuses_an_id_that_points_outside() {
        let root = root();
        for id in ["../etc", "..", "a/b", ""] {
            assert!(remove_plugin(&root, id).is_err(), "{id} must be refused");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scan_ignores_staging_and_dot_directories() {
        let root = root();
        std::fs::create_dir_all(root.join(".staging-abc")).unwrap();
        std::fs::create_dir_all(root.join(".previous-abc")).unwrap();
        install_package(&root, &package(&[("manifest.json", MANIFEST)])).unwrap();
        let scanned = scan_installed(&root);
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].id, "probe");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_empty_root_scans_to_nothing() {
        let root = std::env::temp_dir().join(format!("mfmcl-none-{}", uuid::Uuid::new_v4()));
        assert!(scan_installed(&root).is_empty());
    }
}
