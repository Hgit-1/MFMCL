pub mod aria2;
mod control;
pub use control::DownloadControl;

use futures_util::StreamExt;
use mc_metadata::SourcePreference;
use reqwest::{Client, Url, header};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::Ordering,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    fs,
    io::AsyncSeekExt,
    io::AsyncWriteExt,
    sync::{Mutex, Semaphore, mpsc},
};
use uuid::Uuid;

/// 单个分片的最小字节数。低于这个大小的文件再切分只会增加连接开销。
const MIN_SEGMENT_BYTES: u64 = 256 * 1024;

/// 单个分片的下载结果。
enum SegmentOutcome {
    Written,
    RangeUnsupported,
}

/// 把文件按字节均分成不超过 `segments` 个闭区间，每段至少 [MIN_SEGMENT_BYTES]。
fn segment_ranges(size: u64, segments: usize) -> Vec<(u64, u64)> {
    if size == 0 {
        return Vec::new();
    }
    let max_by_size = (size / MIN_SEGMENT_BYTES).max(1) as usize;
    let count = segments.clamp(1, 16).min(max_by_size);
    let chunk = size.div_ceil(count as u64);
    let mut ranges = Vec::with_capacity(count);
    let mut start = 0u64;
    while start < size {
        let end = (start + chunk - 1).min(size - 1);
        ranges.push((start, end));
        start = end + 1;
    }
    ranges
}

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("download request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("file operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("aria2 download failed: {0}")]
    Aria2(String),
    #[error("no download source is available for {0}")]
    NoSource(String),
    #[error("downloaded file failed integrity validation: {0}")]
    Integrity(String),
    #[error("download cancelled")]
    Cancelled,
    #[error("unsafe download URL or mirror configuration")]
    UnsafeUrl,
    #[error("invalid HTTP range response")]
    InvalidRange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSettings {
    pub source: SourcePreference,
    pub max_concurrent_downloads: u8,
    pub connections_per_file: u8,
    pub aria2_enabled: bool,
}
impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            source: SourcePreference::Official,
            max_concurrent_downloads: 4,
            connections_per_file: 4,
            aria2_enabled: true,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactSpec {
    pub id: String,
    pub label: String,
    pub url: String,
    #[serde(default)]
    pub mirror_url: Option<String>,
    pub destination: PathBuf,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub artifact_id: String,
    pub label: String,
    pub received: u64,
    pub total: Option<u64>,
    pub source: String,
    pub stage: DownloadStage,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStage {
    Connecting,
    Downloading,
    Verifying,
    Completed,
    Failed,
}

type Progress = Option<mpsc::UnboundedSender<DownloadProgress>>;
#[derive(Clone)]
pub struct DownloadEngine {
    client: Client,
    settings: DownloadSettings,
    aria2: Option<aria2::Aria2Session>,
    control: DownloadControl,
    max_file_size: Option<u64>,
    permits: Arc<Semaphore>,
    destinations: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
}

impl DownloadEngine {
    pub fn new(settings: DownloadSettings) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if !safe_url(attempt.url())
                    || attempt.previous().len() >= 10
                    || (attempt
                        .previous()
                        .last()
                        .is_some_and(|u| u.scheme() == "https")
                        && attempt.url().scheme() != "https")
                {
                    attempt.error("unsafe download redirect")
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        Ok(Self {
            client,
            permits: Arc::new(Semaphore::new(
                settings.max_concurrent_downloads.clamp(1, 16) as usize,
            )),
            settings,
            aria2: None,
            control: DownloadControl::default(),
            max_file_size: None,
            destinations: Arc::default(),
        })
    }
    pub fn with_aria2(mut self, session: aria2::Aria2Session) -> Self {
        self.aria2 = Some(session);
        self
    }
    /// Attach an installation-scoped control. Clones retain the shared engine limit.
    pub fn with_max_file_size(mut self, limit: u64) -> Self {
        self.max_file_size = Some(limit);
        self
    }
    pub fn with_control(mut self, control: DownloadControl) -> Self {
        self.control = control;
        self
    }
    pub fn control(&self) -> DownloadControl {
        self.control.clone()
    }
    pub fn settings(&self) -> &DownloadSettings {
        &self.settings
    }

    pub async fn download(
        &self,
        artifact: &ArtifactSpec,
        progress: Progress,
    ) -> Result<PathBuf, DownloadError> {
        let result = self.download_inner(artifact, &progress).await;
        if result.is_err() {
            emit(
                &progress,
                artifact,
                None,
                0,
                artifact.size,
                DownloadStage::Failed,
            );
        }
        result
    }

    async fn download_inner(
        &self,
        artifact: &ArtifactSpec,
        progress: &Progress,
    ) -> Result<PathBuf, DownloadError> {
        if let Some(limit) = self.max_file_size
            && artifact.size.is_some_and(|size| size > limit)
        {
            return Err(DownloadError::Integrity(
                "artifact exceeds configured size limit".into(),
            ));
        }
        let original = Url::parse(&artifact.url)?;
        let pinned = hash_pinned(artifact);
        if !safe_url(&original) || (is_mirror_host(&original) && !pinned) {
            return Err(DownloadError::UnsafeUrl);
        }
        if let Some(mirror) = &artifact.mirror_url
            && !safe_mirror_url(&Url::parse(mirror)?)
        {
            return Err(DownloadError::UnsafeUrl);
        }
        let candidates = source_candidates_with_mirror(
            original,
            artifact.mirror_url.as_deref(),
            self.settings.source,
            pinned,
        );
        self.control.checkpoint().await?;
        // Serialize the same destination in addition to the engine-wide network budget.
        let lock = self
            .destinations
            .lock()
            .await
            .entry(artifact.destination.clone())
            .or_default()
            .clone();
        let _destination = self.control.wait_for(lock.lock()).await?;
        let _permit = self
            .control
            .wait_for(self.permits.acquire())
            .await?
            .expect("engine semaphore remains open");
        if fs::try_exists(&artifact.destination).await?
            && fs::metadata(&artifact.destination)
                .await
                .is_ok_and(|meta| self.max_file_size.is_none_or(|limit| meta.len() <= limit))
            && (pinned || artifact.size.is_some())
            && self
                .control
                .wait_for(verify_file(
                    &artifact.destination,
                    artifact.size,
                    artifact.sha1.as_deref(),
                    artifact.sha256.as_deref(),
                ))
                .await?
                .is_ok()
        {
            emit(
                progress,
                artifact,
                None,
                fs::metadata(&artifact.destination).await?.len(),
                artifact.size,
                DownloadStage::Completed,
            );
            return Ok(artifact.destination.clone());
        }
        if let Some(parent) = artifact
            .destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).await?;
        }
        let temporary = partial_path(artifact);
        let mut last_error = DownloadError::NoSource(artifact.label.clone());
        /*
         * 单文件分段并发的条件是「服务端支持 Range 且总大小已知」。
         *
         * 只有带校验值的条目才允许这么下：分片范围是我们自己算的，最终必须能用
         * 清单里的 SHA 证明整份文件正确，否则宁可退化成单连接顺序下载。
         * aria2 已启用时由 aria2 自己分片，不走这条路径。
         */
        let segments = if self.settings.aria2_enabled
            && self.max_file_size.is_none()
            && self.aria2.is_some()
        {
            1
        } else {
            self.settings.connections_per_file.clamp(1, 16) as usize
        };
        for source in candidates {
            emit(
                progress,
                artifact,
                Some(&source),
                0,
                artifact.size,
                DownloadStage::Connecting,
            );
            if self.settings.aria2_enabled
                && self.max_file_size.is_none()
                && let Some(session) = &self.aria2
            {
                // Separate aria2's opaque fragment state from validated native HTTP state.
                let aria_temporary =
                    temporary.with_extension(format!("aria-{}", Uuid::new_v4().simple()));
                let result = self
                    .download_with_aria2(artifact, &source, session, &aria_temporary, progress)
                    .await;
                match result {
                    Ok(()) => {
                        self.control.checkpoint().await?;
                        replace_file(&aria_temporary, &artifact.destination).await?;
                        cleanup_partial(&temporary).await;
                        emit(
                            progress,
                            artifact,
                            Some(&source),
                            fs::metadata(&artifact.destination).await?.len(),
                            artifact.size,
                            DownloadStage::Completed,
                        );
                        return Ok(artifact.destination.clone());
                    }
                    Err(error) => {
                        // A broken RPC must never leave a writer racing the native fallback.
                        let _ = session.shutdown().await;
                        cleanup_partial(&aria_temporary).await;
                        let _ = fs::remove_file(aria_temporary.with_extension(format!(
                            "{}.aria2",
                            aria_temporary
                                .extension()
                                .unwrap_or_default()
                                .to_string_lossy()
                        )))
                        .await;
                        if matches!(error, DownloadError::Cancelled) {
                            return Err(error);
                        }
                    }
                }
            }
            for attempt in 0..3_u32 {
                self.control.checkpoint().await?;
                match self
                    .download_one(artifact, &source, &temporary, segments, progress)
                    .await
                {
                    Ok(()) => {
                        self.control.checkpoint().await?;
                        replace_file(&temporary, &artifact.destination).await?;
                        let _ = fs::remove_file(metadata_path(&temporary)).await;
                        emit(
                            progress,
                            artifact,
                            Some(&source),
                            fs::metadata(&artifact.destination).await?.len(),
                            artifact.size,
                            DownloadStage::Completed,
                        );
                        return Ok(artifact.destination.clone());
                    }
                    Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
                    Err(error) => {
                        let retry = retryable(&error);
                        if matches!(
                            error,
                            DownloadError::Integrity(_) | DownloadError::InvalidRange
                        ) {
                            cleanup_partial(&temporary).await;
                        }
                        last_error = error;
                        if !retry || attempt == 2 {
                            break;
                        }
                        self.control
                            .wait_for(tokio::time::sleep(Duration::from_millis(
                                200 * (1 << attempt),
                            )))
                            .await?;
                    }
                }
            }
        }
        Err(last_error)
    }

    async fn download_one(
        &self,
        artifact: &ArtifactSpec,
        source: &Url,
        temporary: &Path,
        segments: usize,
        progress: &Progress,
    ) -> Result<(), DownloadError> {
        let pinned = hash_pinned(artifact);
        let old: Option<ResumeMetadata> = fs::read(metadata_path(temporary))
            .await
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let mut offset = fs::metadata(temporary).await.map(|m| m.len()).unwrap_or(0);
        let old = old.filter(|m| {
            (m.source == fingerprint(source.as_str()) || pinned)
                && (m.etag.is_some() || pinned)
                && m.total.is_none_or(|t| offset < t)
        });
        if old.is_none() {
            offset = 0;
        }
        /*
         * 分段并发只在「没有续传进度 + 大小已知 + 有校验值 + 请求了多连接」时启用。
         * 续传已有分片、或大小未知（无法均分）时退回单连接，避免两套状态混用。
         */
        let segment_total = artifact.size.filter(|size| {
            segments > 1 && pinned && offset == 0 && *size > MIN_SEGMENT_BYTES * segments as u64
        });
        /*
         * 分段并发：把文件按字节均分，多条连接同时拉各自的区间，按偏移写进同一份文件。
         *
         * 只有「本次没有任何已有分片」且大小已知时才尝试，所以不会和续传状态混用。
         * 第一个区间的响应同时充当探测：服务端不认 Range（返回 200 而非 206）就
         * 说明它会把整个文件发下来，此时放弃分段、退回单连接，任何服务器都能下载。
         */
        if let Some(size) = segment_total
            && self
                .download_segments(artifact, source, temporary, size, segments, progress)
                .await?
        {
            return Ok(());
        }
        let mut request = self
            .client
            .get(source.clone())
            .header(header::ACCEPT_ENCODING, "identity");
        if offset > 0 {
            request = request.header(header::RANGE, format!("bytes={offset}-"));
            if let Some(etag) = old.as_ref().and_then(|m| m.etag.as_ref()) {
                request = request.header(header::IF_RANGE, etag);
            }
        }
        let response = self.control.wait_for(request.send()).await??;
        if response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            cleanup_partial(temporary).await;
            return Err(DownloadError::InvalidRange);
        }
        let response = response.error_for_status()?;
        let final_source = fingerprint(response.url().as_str());
        let etag = strong_etag(response.headers());
        let mut total = response.content_length();
        if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            let range = response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_range)
                .ok_or(DownloadError::InvalidRange)?;
            if range.0 != offset
                || range.1 < range.0
                || range.2 == 0
                || range.1 != range.2 - 1
                || response
                    .content_length()
                    .is_some_and(|n| n != range.1 - range.0 + 1)
                || artifact.size.is_some_and(|n| n != range.2)
                || (offset > 0
                    && old.as_ref().is_some_and(|m| {
                        m.total.is_some_and(|n| n != range.2)
                            || (!pinned && (m.final_source != final_source || m.etag != etag))
                    }))
            {
                return Err(DownloadError::InvalidRange);
            }
            total = Some(range.2);
        } else if response.status() == reqwest::StatusCode::OK {
            // Server ignored Range or If-Range detected a changed entity: start over.
            offset = 0;
        } else {
            return Err(DownloadError::InvalidRange);
        }
        if response
            .headers()
            .get(header::CONTENT_ENCODING)
            .is_some_and(|v| v != "identity")
        {
            return Err(DownloadError::InvalidRange);
        }
        if artifact.size.zip(total).is_some_and(|(a, b)| a != b) {
            return Err(DownloadError::Integrity(
                "response size differs from metadata".into(),
            ));
        }
        if total
            .or(artifact.size)
            .is_some_and(|length| self.max_file_size.is_some_and(|limit| length > limit))
        {
            return Err(DownloadError::Integrity(
                "response exceeds configured size limit".into(),
            ));
        }
        let metadata = ResumeMetadata {
            source: fingerprint(source.as_str()),
            final_source,
            etag,
            total,
        };
        // Truncate before publishing metadata for a new entity.
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(offset > 0)
            .truncate(offset == 0)
            .open(temporary)
            .await?;
        fs::write(
            metadata_path(temporary),
            serde_json::to_vec(&metadata).expect("resume metadata serialization"),
        )
        .await?;
        let mut stream = response.bytes_stream();
        let mut received = offset;
        while let Some(chunk) = self.control.wait_for(stream.next()).await? {
            let chunk = chunk?;
            received += chunk.len() as u64;
            if total.or(artifact.size).is_some_and(|n| received > n)
                || self.max_file_size.is_some_and(|limit| received > limit)
            {
                return Err(DownloadError::InvalidRange);
            }
            self.control.wait_for(file.write_all(&chunk)).await??;
            emit(
                progress,
                artifact,
                Some(source),
                received,
                total.or(artifact.size),
                DownloadStage::Downloading,
            );
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        if total.is_some_and(|n| n != received) {
            return Err(DownloadError::Integrity("incomplete response".into()));
        }
        emit(
            progress,
            artifact,
            Some(source),
            received,
            total.or(artifact.size),
            DownloadStage::Verifying,
        );
        self.control
            .wait_for(verify_file(
                temporary,
                artifact.size,
                artifact.sha1.as_deref(),
                artifact.sha256.as_deref(),
            ))
            .await??;
        Ok(())
    }

    /// 分段并发下载。
    ///
    /// 返回 `Ok(false)` 表示服务端不支持 Range，调用方应退回单连接下载；
    /// 返回 `Ok(true)` 表示整份文件已按分片写完并通过校验。
    async fn download_segments(
        &self,
        artifact: &ArtifactSpec,
        source: &Url,
        temporary: &Path,
        size: u64,
        segments: usize,
        progress: &Progress,
    ) -> Result<bool, DownloadError> {
        let ranges = segment_ranges(size, segments);
        let metadata = ResumeMetadata {
            source: fingerprint(source.as_str()),
            final_source: fingerprint(source.as_str()),
            etag: None,
            total: Some(size),
        };
        // 先建好文件并发布续传元数据，再并行拉取各区间。
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(temporary)
            .await?;
        file.set_len(size).await?;
        fs::write(
            metadata_path(temporary),
            serde_json::to_vec(&metadata).expect("resume metadata serialization"),
        )
        .await?;
        drop(file);

        let received = Arc::new(std::sync::atomic::AtomicU64::new(0));
        /*
         * 一旦某个分片发现服务端不认 Range，其余分片应立即放弃读取响应体。
         * 否则它们会把整份文件各收一遍（N 倍流量），然后才轮到调用方退回单连接。
         */
        let abort = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut tasks = Vec::with_capacity(ranges.len());
        for (start, end) in ranges {
            let start_byte = start;
            let end_byte = end;
            let engine = self.clone();
            let artifact = artifact.clone();
            let source = source.clone();
            let temporary = temporary.to_path_buf();
            let progress = progress.clone();
            let received = received.clone();
            let abort = abort.clone();
            tasks.push((
                start_byte,
                tokio::spawn(async move {
                    engine
                        .download_segment(
                            &artifact, &source, &temporary, start_byte, end_byte, &progress,
                            &received, &abort,
                        )
                        .await
                }),
            ));
        }
        // 按起始偏移排序是为了让「不支持 Range」的判定优先来自第一个分片。
        tasks.sort_by_key(|(start, _)| *start);
        let mut unsupported = false;
        for (_, task) in tasks {
            match task.await {
                Ok(Ok(SegmentOutcome::Written)) => {}
                Ok(Ok(SegmentOutcome::RangeUnsupported)) => unsupported = true,
                Ok(Err(error)) => {
                    cleanup_partial(temporary).await;
                    return Err(error);
                }
                Err(error) => {
                    cleanup_partial(temporary).await;
                    return Err(DownloadError::Io(std::io::Error::other(format!(
                        "segment task failed: {error}"
                    ))));
                }
            }
        }
        if unsupported {
            cleanup_partial(temporary).await;
            return Ok(false);
        }
        emit(
            progress,
            artifact,
            Some(source),
            size,
            Some(size),
            DownloadStage::Verifying,
        );
        self.control
            .wait_for(verify_file(
                temporary,
                artifact.size,
                artifact.sha1.as_deref(),
                artifact.sha256.as_deref(),
            ))
            .await??;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_segment(
        &self,
        artifact: &ArtifactSpec,
        source: &Url,
        temporary: &Path,
        start: u64,
        end: u64,
        progress: &Progress,
        received: &Arc<std::sync::atomic::AtomicU64>,
        abort: &Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<SegmentOutcome, DownloadError> {
        let response = self
            .control
            .wait_for(
                self.client
                    .get(source.clone())
                    .header(header::ACCEPT_ENCODING, "identity")
                    .header(header::RANGE, format!("bytes={start}-{end}"))
                    .send(),
            )
            .await??;
        if response.status() == reqwest::StatusCode::OK {
            // 服务端忽略 Range，整份文件会从 0 开始发下来：交给单连接路径处理。
            // 通知其余分片立刻停手，避免各自把整份文件收一遍。
            abort.store(true, Ordering::Relaxed);
            return Ok(SegmentOutcome::RangeUnsupported);
        }
        if response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            return Err(DownloadError::InvalidRange);
        }
        let response = response.error_for_status()?;
        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(DownloadError::InvalidRange);
        }
        let range = response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_range)
            .ok_or(DownloadError::InvalidRange)?;
        let expected = end - start + 1;
        if range.0 != start
            || range.2 == 0
            || artifact.size.is_some_and(|size| size != range.2)
            || response
                .content_length()
                .is_some_and(|length| length != expected)
        {
            return Err(DownloadError::InvalidRange);
        }
        if response
            .headers()
            .get(header::CONTENT_ENCODING)
            .is_some_and(|value| value != "identity")
        {
            return Err(DownloadError::InvalidRange);
        }

        let mut file = fs::OpenOptions::new().write(true).open(temporary).await?;
        // 直接把文件游标定位到本分片起点，各分片互不干扰。
        file.seek(std::io::SeekFrom::Start(start)).await?;
        let mut stream = response.bytes_stream();
        let mut written = 0u64;
        while let Some(chunk) = self.control.wait_for(stream.next()).await? {
            if abort.load(Ordering::Relaxed) {
                // 别的分片已判定服务端不支持 Range，本分片不再消耗带宽。
                return Ok(SegmentOutcome::RangeUnsupported);
            }
            let chunk = chunk?;
            written += chunk.len() as u64;
            if written > expected {
                return Err(DownloadError::InvalidRange);
            }
            self.control.wait_for(file.write_all(&chunk)).await??;
            let total_written =
                received.fetch_add(chunk.len() as u64, Ordering::Relaxed) + chunk.len() as u64;
            emit(
                progress,
                artifact,
                Some(source),
                total_written,
                artifact.size,
                DownloadStage::Downloading,
            );
        }
        file.flush().await?;
        if written != expected {
            return Err(DownloadError::Integrity("incomplete response".into()));
        }
        Ok(SegmentOutcome::Written)
    }

    async fn download_with_aria2(
        &self,
        artifact: &ArtifactSpec,
        source: &Url,
        session: &aria2::Aria2Session,
        temporary: &Path,
        progress: &Progress,
    ) -> Result<(), DownloadError> {
        self.control.checkpoint().await?;
        let checksum = artifact
            .sha256
            .as_ref()
            .map(|h| format!("sha-256={h}"))
            .or_else(|| artifact.sha1.as_ref().map(|h| format!("sha-1={h}")));
        // Unpinned artifacts use native HTTP: aria2 cannot enforce our redirect/entity policy.
        if checksum.is_none() {
            return Err(DownloadError::Aria2("native validation required".into()));
        }
        let gid = session
            .add_uri_bounded(
                &[source.to_string()],
                temporary.parent().unwrap_or(Path::new(".")),
                temporary
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or(DownloadError::UnsafeUrl)?,
                checksum.as_deref(),
                self.settings.connections_per_file.clamp(1, 16),
            )
            .await
            .map_err(|_| DownloadError::Aria2("RPC unavailable".into()))?;
        let _cleanup = session.task_guard(gid.clone());
        let started = std::time::Instant::now();
        loop {
            if self.control.is_cancelled() {
                let _ = session.remove(&gid).await;
                return Err(DownloadError::Cancelled);
            }
            if self.control.is_paused() {
                session
                    .pause(&gid)
                    .await
                    .map_err(|_| DownloadError::Aria2("RPC unavailable".into()))?;
                if let Err(error) = self.control.checkpoint().await {
                    let _ = session.remove(&gid).await;
                    return Err(error);
                }
                session
                    .unpause(&gid)
                    .await
                    .map_err(|_| DownloadError::Aria2("RPC unavailable".into()))?;
            }
            if started.elapsed() > Duration::from_secs(1800) {
                return Err(DownloadError::Aria2("download timed out".into()));
            }
            let status = session
                .tell_status(&gid)
                .await
                .map_err(|_| DownloadError::Aria2("RPC unavailable".into()))?;
            emit(
                progress,
                artifact,
                Some(source),
                status.completed_length.parse().unwrap_or(0),
                status
                    .total_length
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .or(artifact.size),
                DownloadStage::Downloading,
            );
            match status.status.as_str() {
                "complete" => break,
                "error" | "removed" => return Err(DownloadError::Aria2("transfer failed".into())),
                _ => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        self.control
            .wait_for(verify_file(
                temporary,
                artifact.size,
                artifact.sha1.as_deref(),
                artifact.sha256.as_deref(),
            ))
            .await??;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct ResumeMetadata {
    source: String,
    final_source: String,
    etag: Option<String>,
    total: Option<u64>,
}
fn fingerprint(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
fn hash_pinned(a: &ArtifactSpec) -> bool {
    a.sha1.is_some() || a.sha256.is_some()
}
fn partial_path(a: &ArtifactSpec) -> PathBuf {
    let identity = if hash_pinned(a) { "" } else { &a.url };
    let key = fingerprint(&format!(
        "{identity}\0{:?}\0{:?}\0{:?}",
        a.size, a.sha1, a.sha256
    ));
    let mut name = a.destination.as_os_str().to_os_string();
    name.push(format!(".mfmcl-{}.part", &key[..24]));
    PathBuf::from(name)
}
fn metadata_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(".json");
    p.into()
}
async fn cleanup_partial(path: &Path) {
    let _ = fs::remove_file(path).await;
    let _ = fs::remove_file(metadata_path(path)).await;
}
fn strong_etag(headers: &header::HeaderMap) -> Option<String> {
    let value = headers.get(header::ETAG)?.to_str().ok()?;
    (value.starts_with('"') && value.ends_with('"') && value.len() >= 2).then(|| value.to_owned())
}
fn parse_range(value: &str) -> Option<(u64, u64, u64)> {
    let (span, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = span.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?, total.parse().ok()?))
}
fn retryable(error: &DownloadError) -> bool {
    match error {
        DownloadError::Network(e) => e.status().is_none_or(|s| {
            s.is_server_error() || s.as_u16() == 408 || s.as_u16() == 429 || s.as_u16() == 416
        }),
        DownloadError::Integrity(_) | DownloadError::InvalidRange => true,
        _ => false,
    }
}
fn emit(
    progress: &Progress,
    a: &ArtifactSpec,
    source: Option<&Url>,
    received: u64,
    total: Option<u64>,
    stage: DownloadStage,
) {
    if let Some(tx) = progress {
        let source = source
            .map(|url| {
                let mut url = url.clone();
                url.set_query(None);
                url.set_fragment(None);
                url.to_string()
            })
            .unwrap_or_default();
        let _ = tx.send(DownloadProgress {
            artifact_id: a.id.clone(),
            label: a.label.clone(),
            received,
            total,
            source,
            stage,
        });
    }
}

/// POSIX replacement is atomic. Windows uses a unique backup and rollback without deleting the old file first.
async fn replace_file(temporary: &Path, destination: &Path) -> Result<(), DownloadError> {
    #[cfg(not(windows))]
    {
        fs::rename(temporary, destination).await?;
    }
    #[cfg(windows)]
    {
        if fs::try_exists(destination).await? {
            let mut backup = destination.as_os_str().to_os_string();
            backup.push(format!(".backup-{}", Uuid::new_v4().simple()));
            let backup = PathBuf::from(backup);
            fs::rename(destination, &backup).await?;
            if let Err(error) = fs::rename(temporary, destination).await {
                if let Err(rollback) = fs::rename(&backup, destination).await {
                    return Err(DownloadError::Io(std::io::Error::other(format!(
                        "replacement failed: {error}; rollback failed: {rollback}; old file retained at {}",
                        backup.display()
                    ))));
                }
                return Err(error.into());
            }
            let _ = fs::remove_file(backup).await;
        } else {
            fs::rename(temporary, destination).await?;
        }
    }
    Ok(())
}
fn safe_url(url: &Url) -> bool {
    if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    match url.scheme() {
        "https" => true,
        // Plain HTTP is reserved for loopback fixture servers in unit tests.
        "http" if cfg!(test) => url.host().is_some_and(|host| match host {
            url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
            url::Host::Ipv4(address) => address.is_loopback(),
            url::Host::Ipv6(address) => address.is_loopback(),
        }),
        _ => false,
    }
}
fn safe_mirror_url(url: &Url) -> bool {
    safe_url(url) && (url.scheme() == "http" || (url.port().is_none() && is_mirror_host(url)))
}
fn is_mirror_host(url: &Url) -> bool {
    url.host_str()
        .is_some_and(|h| h == "bmclapi2.bangbang93.com" || h == "bmclapi.bangbang93.com")
}

pub fn source_candidates(
    original: Url,
    preference: SourcePreference,
    checksum_available: bool,
) -> Vec<Url> {
    source_candidates_with_mirror(original, None, preference, checksum_available)
}
pub fn source_candidates_with_mirror(
    original: Url,
    explicit_mirror: Option<&str>,
    preference: SourcePreference,
    checksum_available: bool,
) -> Vec<Url> {
    if !safe_url(&original) || (is_mirror_host(&original) && !checksum_available) {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    if preference != SourcePreference::Official
        && checksum_available
        && let Some(mirror) = explicit_mirror
            .and_then(|s| Url::parse(s).ok())
            .filter(safe_mirror_url)
            .or_else(|| bmclapi_url(&original))
    {
        candidates.push(mirror);
    }
    if !candidates.contains(&original) {
        candidates.push(original);
    }
    candidates
}
/// Maps exact known Mojang hosts; credentials, custom ports and unrelated hosts are never rewritten.
pub fn bmclapi_url(original: &Url) -> Option<Url> {
    if !safe_url(original) || original.port().is_some() {
        return None;
    }
    let mut mirror = Url::parse("https://bmclapi2.bangbang93.com/").ok()?;
    let path = original.path();
    let mapped = match original.host_str()? {
        "resources.download.minecraft.net" => format!("/assets{path}"),
        "libraries.minecraft.net" => format!("/libraries{path}"),
        "piston-meta.mojang.com" if path == "/mc/game/version_manifest_v2.json" => {
            "/mc/game/version_manifest.json".into()
        }
        "piston-meta.mojang.com"
            if path.starts_with("/v1/packages/") && path.ends_with(".json") =>
        {
            format!("/version/{}", path.rsplit('/').next()?)
        }
        _ => return None,
    };
    mirror.set_path(&mapped);
    mirror.set_query(original.query());
    Some(mirror)
}

pub async fn verify_file(
    path: &Path,
    expected_size: Option<u64>,
    expected_sha1: Option<&str>,
    expected_sha256: Option<&str>,
) -> Result<(), DownloadError> {
    let metadata = fs::metadata(path).await?;
    if expected_size.is_some_and(|n| n != metadata.len()) {
        return Err(DownloadError::Integrity(format!(
            "{} byte count mismatch",
            path.display()
        )));
    }
    if expected_sha1.is_none() && expected_sha256.is_none() {
        return Ok(());
    }
    let mut file = fs::File::open(path).await?;
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut buffer = vec![0_u8; 128 * 1024];
    loop {
        let read = tokio::io::AsyncReadExt::read(&mut file, &mut buffer).await?;
        if read == 0 {
            break;
        }
        sha1.update(&buffer[..read]);
        sha256.update(&buffer[..read]);
    }
    if expected_sha1.is_some_and(|h| !hex::encode(sha1.finalize()).eq_ignore_ascii_case(h)) {
        return Err(DownloadError::Integrity(format!(
            "{} SHA-1 mismatch",
            path.display()
        )));
    }
    if expected_sha256.is_some_and(|h| !hex::encode(sha256.finalize()).eq_ignore_ascii_case(h)) {
        return Err(DownloadError::Integrity(format!(
            "{} SHA-256 mismatch",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
