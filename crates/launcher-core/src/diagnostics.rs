use crate::InstanceRecord;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 24 * 1024 * 1024;
const MAX_FILES: usize = 10;

#[derive(Debug, Error)]
pub enum DiagnosticError {
    #[error("file operation failed: {0}")]
    Io(#[from] io::Error),
    #[error("could not create diagnostic archive: {0}")]
    Archive(String),
    #[error("selected export directory does not exist")]
    InvalidDestination,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSummary {
    pub title: String,
    pub confidence: String,
    pub explanation: String,
    pub evidence: Vec<String>,
    pub files_found: usize,
}

#[derive(Debug, Clone)]
struct DiagnosticFile {
    path: PathBuf,
    archive_name: String,
    modified: Option<std::time::SystemTime>,
}

pub fn summarize_instance(instance: &InstanceRecord) -> Result<DiagnosticSummary, DiagnosticError> {
    let files = discover_logs(instance);
    let mut evidence = Vec::new();
    let mut signatures = Vec::<(&str, &str, &str)>::new();
    signatures.push((
        "OutOfMemoryError",
        "内存不足",
        "游戏日志包含 Java 内存不足错误。",
    ));
    signatures.push((
        "Could not reserve enough space",
        "Java 内存分配失败",
        "Java 无法按当前设置申请所需内存。",
    ));
    signatures.push((
        "NoClassDefFoundError",
        "依赖缺失或不兼容",
        "日志中出现类缺失错误；可能与模组、依赖或游戏版本不匹配有关。",
    ));
    signatures.push((
        "ClassNotFoundException",
        "依赖缺失或不兼容",
        "日志中出现类未找到错误；可能与模组或依赖配置有关。",
    ));
    signatures.push((
        "UnsupportedClassVersionError",
        "Java 版本不匹配",
        "发现类文件与当前 Java 版本不兼容的迹象。",
    ));
    signatures.push((
        "Mixin apply failed",
        "模组兼容性冲突",
        "模组注入过程失败；可能存在版本冲突。",
    ));
    signatures.push((
        "GL_OUT_OF_MEMORY",
        "图形资源不足",
        "渲染日志报告图形资源不足。",
    ));
    signatures.push((
        "EXCEPTION_ACCESS_VIOLATION",
        "原生组件异常退出",
        "进程发生原生访问冲突；驱动或原生依赖可能相关。",
    ));
    let mut matched = None;
    for file in files.iter().take(5) {
        let text = read_bounded(&file.path, MAX_FILE_BYTES)?;
        for (needle, title, explanation) in &signatures {
            if text.contains(needle) {
                evidence.push(format!("{}：检测到关键字 {}", file.archive_name, needle));
                matched = Some((*title, *explanation));
                break;
            }
        }
        if evidence.len() >= 3 {
            break;
        }
    }
    let (title, confidence, explanation) = match matched {
        Some((title, explanation)) => (
            title.to_owned(),
            "low".to_owned(),
            format!(
                "{} 这是基于日志关键字的初步线索，不一定是根本原因。",
                explanation
            ),
        ),
        None if files.is_empty() => (
            "没有找到可分析的日志".to_owned(),
            "none".to_owned(),
            "游戏目录中没有找到近期日志或崩溃报告。".to_owned(),
        ),
        None => (
            "暂时无法确定原因".to_owned(),
            "unknown".to_owned(),
            "已找到日志，但未命中当前已知诊断规则。请查看原始日志或导出诊断包。".to_owned(),
        ),
    };
    Ok(DiagnosticSummary {
        title,
        confidence,
        explanation,
        evidence,
        files_found: files.len(),
    })
}

pub fn export_instance_diagnostics(
    instance: &InstanceRecord,
    destination_dir: &Path,
) -> Result<PathBuf, DiagnosticError> {
    let canonical_destination = destination_dir
        .canonicalize()
        .map_err(|_| DiagnosticError::InvalidDestination)?;
    if !canonical_destination.is_dir() {
        return Err(DiagnosticError::InvalidDestination);
    }
    let files = discover_logs(instance);
    let archive_path = unique_archive_path(&canonical_destination);
    let file = File::create(&archive_path)?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o600);
    let mut total = 0_u64;
    for item in files.iter().take(MAX_FILES) {
        let metadata = match std::fs::metadata(&item.path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        if total.saturating_add(metadata.len()) > MAX_TOTAL_BYTES {
            break;
        }
        let content = read_bounded(&item.path, MAX_FILE_BYTES)?;
        let redacted = redact(&content);
        archive
            .start_file(&item.archive_name, options)
            .map_err(|error| DiagnosticError::Archive(error.to_string()))?;
        archive
            .write_all(redacted.as_bytes())
            .map_err(DiagnosticError::Io)?;
        total = total.saturating_add(redacted.len() as u64);
    }
    archive
        .start_file("diagnostic-summary.txt", options)
        .map_err(|error| DiagnosticError::Archive(error.to_string()))?;
    let summary = summarize_instance(instance)?;
    writeln!(archive, "MFMCL diagnostic summary")?;
    writeln!(archive, "Instance: {}", safe_name(&instance.name))?;
    writeln!(archive, "Minecraft: {}", safe_name(&instance.game_version))?;
    writeln!(
        archive,
        "Finding: {} ({})",
        summary.title, summary.confidence
    )?;
    writeln!(archive, "{}", summary.explanation)?;
    for item in summary.evidence {
        writeln!(archive, "- {}", item)?;
    }
    writeln!(
        archive,
        "\nOnly local log snippets are included. Credentials are redacted."
    )?;
    archive
        .finish()
        .map_err(|error| DiagnosticError::Archive(error.to_string()))?;
    Ok(archive_path)
}

fn discover_logs(instance: &InstanceRecord) -> Vec<DiagnosticFile> {
    let mut files = Vec::new();
    add_candidate(
        &mut files,
        &instance.game_dir.join("logs").join("latest.log"),
        "game-latest.log",
    );
    let crash_dir = instance.game_dir.join("crash-reports");
    add_directory_files(&mut files, &crash_dir, "crash-reports", |name| {
        name.ends_with(".txt")
    });
    add_directory_files(&mut files, &instance.game_dir, "game", |name| {
        name.starts_with("hs_err_pid") && name.ends_with(".log")
    });
    add_candidate(
        &mut files,
        &instance.game_dir.join("debug.log"),
        "debug.log",
    );
    add_directory_files(
        &mut files,
        &instance.instance_dir.join("logs"),
        "launcher",
        |name| name.ends_with(".log"),
    );
    files.sort_by(|left, right| right.modified.cmp(&left.modified));
    files
}

fn add_candidate(files: &mut Vec<DiagnosticFile>, path: &Path, archive_name: &str) {
    let metadata = std::fs::symlink_metadata(path).ok();
    let modified = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok());
    if metadata.is_some_and(|metadata| metadata.file_type().is_file()) {
        files.push(DiagnosticFile {
            path: path.to_path_buf(),
            archive_name: archive_name.to_owned(),
            modified,
        });
    }
}

fn add_directory_files<F>(
    files: &mut Vec<DiagnosticFile>,
    directory: &Path,
    archive_prefix: &str,
    predicate: F,
) where
    F: Fn(&str) -> bool,
{
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if files.len() >= MAX_FILES * 2 {
            break;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        let Some(name) = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        if !predicate(&name) {
            continue;
        }
        let modified = std::fs::metadata(&path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        files.push(DiagnosticFile {
            path,
            archive_name: format!("{archive_prefix}/{name}"),
            modified,
        });
    }
}

fn read_bounded(path: &Path, max_bytes: u64) -> Result<String, DiagnosticError> {
    let file = File::open(path)?;
    let mut buffer = Vec::new();
    file.take(max_bytes).read_to_end(&mut buffer)?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

fn redact(text: &str) -> String {
    crate::logs::redact_log(text, &[])
}

fn unique_archive_path(directory: &Path) -> PathBuf {
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let mut candidate = directory.join(format!("MFMCL-diagnostics-{timestamp}.zip"));
    let mut suffix = 1;
    while candidate.exists() {
        candidate = directory.join(format!("MFMCL-diagnostics-{timestamp}-{suffix}.zip"));
        suffix += 1;
    }
    candidate
}

fn safe_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(100)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::ZipArchive;

    #[test]
    fn summarizes_and_exports_bounded_redacted_logs() {
        let root =
            std::env::temp_dir().join(format!("mfmcl-diagnostic-test-{}", uuid::Uuid::new_v4()));
        let game_dir = root.join("game");
        let destination = root.join("export");
        std::fs::create_dir_all(game_dir.join("logs")).unwrap();
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(
            game_dir.join("logs/latest.log"),
            "java.lang.OutOfMemoryError: Java heap space\naccessToken=secret-token\n",
        )
        .unwrap();
        let instance = InstanceRecord {
            id: "test-instance".to_owned(),
            name: "Test".to_owned(),
            game_version: "1.21.4".to_owned(),
            loader: "vanilla".to_owned(),
            instance_dir: root.join("instance"),
            game_dir,
            isolated: true,
            state: crate::InstanceState::Failed,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
        };
        let summary = summarize_instance(&instance).unwrap();
        assert_eq!(summary.title, "内存不足");
        let archive_path = export_instance_diagnostics(&instance, &destination).unwrap();
        let file = File::open(&archive_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut log = String::new();
        archive
            .by_name("game-latest.log")
            .unwrap()
            .read_to_string(&mut log)
            .unwrap();
        assert!(!log.contains("secret-token"));
        assert!(log.contains("[REDACTED]"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn redacts_common_credentials() {
        let result = redact("accessToken=secret123 username=Alex");
        assert!(!result.contains("secret123"));
        assert!(result.contains("[REDACTED]"));
        assert!(result.contains("username=Alex"));
        let json_result = redact(r#"{"accessToken":"secret-json", "name":"Alex"}"#);
        assert!(!json_result.contains("secret-json"));
        let bearer_result = redact("Authorization: Bearer secret-bearer");
        assert!(!bearer_result.contains("secret-bearer"));
    }
}
