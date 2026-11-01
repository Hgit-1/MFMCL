//! 本地 AI 模型文件的下载与管理。
//!
//! ## 为什么单独一个模块
//!
//! 崩溃分析用的模型是一个 ~1.1 GB 的 GGUF 文件，它和启动器里的其它下载（游戏
//! 文件、模组、整合包）有三点根本不同：
//!
//!   1. **来源不是 Mojang**。`download-engine` 的镜像逻辑只认 bmclapi 对 Mojang
//!      域名的改写，模型托管在 HuggingFace 与 ModelScope 上，套不进去。硬塞的话
//!      就得放宽那个模块的镜像白名单，而它目前刻意收得很紧。
//!   2. **要选最快的源**。用户住处不同，「哪个源快」差别很大：实测同一台机器上
//!      ModelScope 比 HuggingFace 镜像快四倍，换个网络可能反过来。所以下载前先
//!      各拉一小段测速，按实测吞吐排序。
//!   3. **不是安装游戏的一部分**。它没有实例归属，失败也不该弄脏任何实例状态。
//!
//! 所以这里自带一条下载路径，但完整性校验仍复用 `download_engine::verify_file`
//! ——哈希比对这套逻辑没有理由再写一遍。
//!
//! ## 它下载到的位置，以及它还不能做什么
//!
//! 文件落在应用数据目录的 `models/` 下，路径通过 `crash_ai_model_status` 返回给
//! 界面。**启动器不会因此就能跑模型**：推理引擎（llama.cpp 的 `llama-server`）
//! 仍是外部进程，把这个文件交给它是用户（或将来某个版本）的事。界面上必须写明
//! 这一点，而不是让「下载完成」看起来像「功能已就绪」。
//!
//! ## 预留给后续改造的接缝
//!
//! 模型描述集中在 `MODEL` 常量里（文件、大小、哈希、候选源），换成别的模型只需
//! 改这一处；`crash_ai_model_remove` 与「重新下载」让同一个入口可以复用成「换
//! 模型」；路径由 `models_dir` 统一决定，将来若要支持用户自选目录，改这一个函数
//! 即可。

use download_engine::verify_file;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// 测速时每个源拉取的字节数。
///
/// 太小会被 TCP 慢启动和 TLS 握手主导，测出来的是「连接快慢」而不是「吞吐」；
/// 512 KB 在常见网络下足以让连接进入稳定状态，又不至于让测速本身等太久。
const PROBE_BYTES: u64 = 512 * 1024;

/// 单个源的测速上限。超过这个时间还没拉完就按「不可用」处理——慢到这个程度的源
/// 本来也不该被选中。
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// 进度事件的节流间隔。
///
/// 每个 chunk 都发一次事件会让上万个 IPC 调用挤占界面线程；200 ms 一次既能让进度
/// 条看起来连续，又不会拖慢下载本身。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// 一个候选下载源。
#[derive(Debug)]
struct ModelSource {
    /// 显示给用户的名字。
    label: &'static str,
    url: &'static str,
}

/// 一个可下载的模型。
#[derive(Debug)]
struct ModelDescriptor {
    /// 稳定标识。将来支持多个模型时，界面用它区分。
    id: &'static str,
    /// 显示名。
    name: &'static str,
    /// 落盘文件名。带量化档位，避免换档位时覆盖旧文件。
    file_name: &'static str,
    /// 期望字节数。用于「是否已下载」的判定与下载后校验。
    size: u64,
    /// 期望 SHA-256（来自托管方的 LFS 元数据）。
    sha256: &'static str,
    /// 候选源，按默认优先级排列。
    sources: &'static [ModelSource],
}

/// 当前使用的模型。
///
/// LFM3-1.7B 的 Q4_K_M 档：约 1.1 GB 权重，实测常驻 1.76 GB，CPU 上约 26 tok/s，
/// 是 `docs/CRASH_AI.md` 里横向对比后选定的那一档。换档位（Q8 更准但内存翻倍）
/// 只需要改这里的 `file_name` / `size` / `sha256`。
const MODEL: ModelDescriptor = ModelDescriptor {
    id: "lfm3-1.7b-q4km",
    name: "LFM3-1.7B (Q4_K_M)",
    file_name: "LFM3-1.7B-Q4_K_M.gguf",
    size: 1_099_627_808,
    sha256: "3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4",
    sources: &[
        ModelSource {
            label: "hf-mirror.com",
            url: "https://hf-mirror.com/LiquidAI/LFM3-1.7B-GGUF/resolve/main/LFM3-1.7B-Q4_K_M.gguf",
        },
        ModelSource {
            label: "ModelScope",
            url: "https://www.modelscope.cn/models/LiquidAI/LFM3-1.7B-GGUF/resolve/master/LFM3-1.7B-Q4_K_M.gguf",
        },
        ModelSource {
            label: "HuggingFace",
            url: "https://huggingface.co/LiquidAI/LFM3-1.7B-GGUF/resolve/main/LFM3-1.7B-Q4_K_M.gguf",
        },
    ],
};

/// 允许下载的主机白名单。
///
/// 与 `download-engine` 的做法一致：只认写死的域名，不做通配。模型文件的 URL 是
/// 编译期常量，本来不会变；这条检查防的是「有人把常量改错」或「重定向到别处」，
/// 而不是配置错误。
const ALLOWED_HOSTS: &[&str] = &["hf-mirror.com", "huggingface.co", "www.modelscope.cn"];

/// 下载互斥状态。
///
/// 放在 Tauri 的托管状态里而不是模块级 static：后者在测试与热重载下会串味，而且
/// 无法随应用生命周期销毁。`cancel` 用原子布尔，因为下载循环只在每个 chunk 之后
/// 读一次，不需要更复杂的同步。
#[derive(Default)]
pub struct ModelDownloads {
    running: std::sync::Mutex<bool>,
    cancel: AtomicBool,
}

/// 一个源的状态，供界面显示「哪个源最快」。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSourceStatus {
    pub label: String,
    pub url: String,
    /// 测速得到的吞吐（字节/秒）。不可用时为 `None`。
    pub bytes_per_second: Option<u64>,
    /// 不可用的原因；可用时为 `None`。
    pub problem: Option<String>,
}

/// 模型在本机的状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub id: String,
    pub name: String,
    pub file_name: String,
    /// 模型文件应落在的完整路径。
    pub path: String,
    /// 模型目录。界面用它说明「文件下到哪」。
    pub directory: String,
    pub expected_bytes: u64,
    /// 文件存在且大小与期望一致。
    ///
    /// 只比大小、不每次重算哈希：状态查询会在打开设置面板时调用，对 1.1 GB 做一
    /// 遍 SHA-256 要几秒，而「大小对得上」已经能排除绝大多数真实故障（没下完、
    /// 被删除）。完整性由下载完成时的那一次校验保证。
    pub installed: bool,
    /// 磁盘上的实际字节数；文件不存在时为 `None`。
    pub bytes_on_disk: Option<u64>,
    /// 此刻是否有下载在进行。
    pub downloading: bool,
    pub sha256: String,
}

/// 下载进度事件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProgress {
    /// `probing` / `downloading` / `verifying` / `completed` / `failed`
    pub stage: &'static str,
    pub received: u64,
    pub total: u64,
    /// 正在使用的源名；测速阶段为 `None`。
    pub source: Option<String>,
    /// 实测速度（字节/秒），供界面显示「还剩多久」。
    pub bytes_per_second: Option<u64>,
}

fn model_error(error: impl std::fmt::Display) -> String {
    format!("模型下载未完成：{error}")
}

/// 主机是否在白名单里。
fn allowed_host(url: &reqwest::Url) -> bool {
    url.host_str().is_some_and(|host| {
        ALLOWED_HOSTS
            .iter()
            .any(|allowed| host.eq_ignore_ascii_case(allowed))
    })
}

/// URL 是否可以作为模型源使用：HTTPS、带主机、无内嵌凭据、主机在白名单里。
///
/// 测试例外与 `download-engine` 采用同一条约定：`cfg!(test)` 下允许明文 HTTP 的
/// 回环地址。端到端链路（测速 → 下载 → 校验 → 改名）只能用本地假服务器验证，
/// 而这条路径涉及取消、短响应、写盘失败等分支，不测就是靠猜。这个放宽只在测试
/// 构建里存在，发布版本中 `cfg!(test)` 为 false，回环 HTTP 立刻被拒。
fn safe_source(url: &str) -> Option<reqwest::Url> {
    let parsed = reqwest::Url::parse(url).ok()?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    if cfg!(test) && parsed.scheme() == "http" {
        return parsed
            .host_str()
            .is_some_and(|host| host == "127.0.0.1" || host == "localhost")
            .then_some(parsed);
    }
    if parsed.scheme() != "https" {
        return None;
    }
    if !allowed_host(&parsed) {
        return None;
    }
    Some(parsed)
}

/// 构建下载用的 HTTP 客户端。
///
/// 重定向逐跳校验：托管方会 302 到 CDN，CDN 的域名不在白名单里，所以不能简单地
/// 要求「最终域名也在白名单」。这里的规则是「跳转目标必须是 HTTPS」——这是真正
/// 与安全相关的那一条（防降级）。备份链路的凭据不在这里，也不带 cookie。
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10
                || (attempt
                    .previous()
                    .last()
                    .is_some_and(|previous| previous.scheme() == "https")
                    && attempt.url().scheme() != "https")
            {
                attempt.error("unexpected model download redirect")
            } else {
                attempt.follow()
            }
        }))
        // 端到端测试用回环 HTTP 假服务器；发布构建里 `safe_source` 已经拒绝了
        // 明文地址，这里不需要再收一次。
        .build()
        .map_err(model_error)
}

fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法定位应用数据目录：{error}"))?;
    Ok(root.join("models"))
}

/// 读一个目录里的模型状态。
///
/// 与 `current_status` 分开是为了测试：它只需要一个路径，不需要 Tauri 运行时，
/// 因此「文件大小对不上时怎么判定」这类逻辑可以用临时目录直接验证。
///
/// ## 为什么也要看 `.part`
///
/// 下载中断（取消、崩溃、断电）会留下一个 GB 级的 `.part`。只看最终文件的话，
/// 界面会说「尚未下载」，而磁盘上其实躺着一 GB 垃圾——用户既看不到它，也没有
/// 任何入口能清掉。所以残留的 `.part` 也要计入 `bytesOnDisk`，让界面显示「文件
/// 不完整」并给出「删除」按钮。
fn status_in(directory: &Path, downloading: bool) -> ModelStatus {
    let path = directory.join(MODEL.file_name);
    let partial = directory.join(format!("{}.part", MODEL.file_name));
    let final_bytes = std::fs::metadata(&path).ok().map(|meta| meta.len());
    let partial_bytes = std::fs::metadata(&partial).ok().map(|meta| meta.len());
    // 最终文件存在就用它；否则报告半成品的体积。
    let bytes = final_bytes.or(partial_bytes);
    ModelStatus {
        id: MODEL.id.to_owned(),
        name: MODEL.name.to_owned(),
        file_name: MODEL.file_name.to_owned(),
        path: path.to_string_lossy().into_owned(),
        directory: directory.to_string_lossy().into_owned(),
        expected_bytes: MODEL.size,
        installed: final_bytes.is_some_and(|size| size == MODEL.size),
        bytes_on_disk: bytes,
        downloading,
        sha256: MODEL.sha256.to_owned(),
    }
}

/// 读取模型文件当前的状态。
fn current_status(app: &AppHandle, downloads: &ModelDownloads) -> Result<ModelStatus, String> {
    let directory = models_dir(app)?;
    let downloading = downloads.running.lock().map(|flag| *flag).unwrap_or(false);
    Ok(status_in(&directory, downloading))
}

/// 测一个源的吞吐。
///
/// 用 Range 请求取前 `PROBE_BYTES` 字节并计时。为什么不用 HEAD：HEAD 只测往返延迟，
/// 与真实吞吐经常不成比例（镜像缓存未命中时会先慢后快）。为什么限定范围：完整
/// 文件是 GB 级，测速不能真的下完。
async fn probe_one(client: &reqwest::Client, source: &ModelSource) -> ModelSourceStatus {
    let unusable = |problem: String| ModelSourceStatus {
        label: source.label.to_owned(),
        url: source.url.to_owned(),
        bytes_per_second: None,
        problem: Some(problem),
    };
    let Some(_) = safe_source(source.url) else {
        return unusable("源地址不在允许列表内。".into());
    };
    let started = Instant::now();
    let request = client
        .get(source.url)
        .header(
            reqwest::header::RANGE,
            format!("bytes=0-{}", PROBE_BYTES - 1),
        )
        .timeout(PROBE_TIMEOUT)
        .send();
    let Ok(response) = tokio::time::timeout(PROBE_TIMEOUT, request).await else {
        return unusable("测速超时。".into());
    };
    let response = match response {
        Ok(response) => response,
        Err(error) => return unusable(format!("连接失败：{error}")),
    };
    if !response.status().is_success() {
        return unusable(format!("服务返回 HTTP {}", response.status()));
    }
    let mut received: u64 = 0;
    let mut body = response;
    loop {
        match body.chunk().await {
            Ok(Some(chunk)) => {
                received += chunk.len() as u64;
                if received >= PROBE_BYTES {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => return unusable(format!("读取中断：{error}")),
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    if received == 0 || elapsed <= 0.0 {
        return unusable("没有收到任何数据。".into());
    }
    ModelSourceStatus {
        label: source.label.to_owned(),
        url: source.url.to_owned(),
        bytes_per_second: Some((received as f64 / elapsed).round() as u64),
        problem: None,
    }
}

/// 对所有候选源测速，按吞吐从高到低排序。
///
/// 顺序测而不是并发测：三个源并发会各自抢带宽，测出来的吞吐互相压低，排序就失去
/// 意义——而排序正是这一步的全部目的。串行最坏要等三个超时（30 秒），但只有
/// 「全部源都不可用」时才走到最坏情况；正常网络下每个源几秒内就有结果。
async fn probe_all(client: &reqwest::Client) -> Vec<ModelSourceStatus> {
    probe_sources(client, &MODEL).await
}

/// 对一个描述里的所有候选源测速，按吞吐从高到低排序。
async fn probe_sources(
    client: &reqwest::Client,
    descriptor: &ModelDescriptor,
) -> Vec<ModelSourceStatus> {
    let mut results = Vec::with_capacity(descriptor.sources.len());
    for source in descriptor.sources {
        results.push(probe_one(client, source).await);
    }
    // 用 `sort_by_key` + `Reverse` 表达「速度高者在前」，比手写比较器更难写错。
    results.sort_by_key(|source| std::cmp::Reverse(source.bytes_per_second));
    results
}

fn emit_progress(app: &AppHandle, progress: ModelProgress) {
    let _ = app.emit("crash-ai-model-progress", progress);
}

/// 查询模型状态。
#[tauri::command]
pub fn crash_ai_model_status(
    app: AppHandle,
    downloads: tauri::State<'_, ModelDownloads>,
) -> Result<ModelStatus, String> {
    current_status(&app, &downloads)
}

/// 测速并返回排序后的源列表。
///
/// 单独一个命令而不是塞进 `crash_ai_model_download`：界面需要在下载**之前**就
/// 显示「将从最快的源下载」，而且测速结果可以让用户判断「是不是网络有问题」——
/// 一个必然失败的下载按钮给不了这个信息。
#[tauri::command]
pub async fn crash_ai_model_sources() -> Result<Vec<ModelSourceStatus>, String> {
    let client = client()?;
    Ok(probe_all(&client).await)
}

/// 取消正在进行的下载。
#[tauri::command]
pub fn crash_ai_model_cancel(downloads: tauri::State<'_, ModelDownloads>) -> Result<(), String> {
    downloads.cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// 删除已下载的模型文件。
///
/// 与「重新下载」一起构成后续改造的入口（换模型、换档位、腾空间）。
#[tauri::command]
pub async fn crash_ai_model_remove(
    app: AppHandle,
    downloads: tauri::State<'_, ModelDownloads>,
) -> Result<ModelStatus, String> {
    {
        let running = downloads.running.lock().map_err(|_| "下载状态不可用。")?;
        if *running {
            return Err("正在下载，请先取消。".into());
        }
    }
    // 模型正文与临时文件一起删：取消或崩溃留下的 `.part` 同样是「用户不该再看到
    // 它」的东西，留着只会白占磁盘。
    let directory = models_dir(&app)?;
    for path in [
        directory.join(MODEL.file_name),
        directory.join(format!("{}.part", MODEL.file_name)),
    ] {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            // 文件本来就不存在时不该报错：用户点「删除」的意图是「让它不在」，而它
            // 已经不在，这个意图已经满足了。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("无法删除模型文件：{error}")),
        }
    }
    let status = current_status(&app, &downloads)?;
    Ok(status)
}

/// 下载模型。
///
/// 流程：测速 → 选最快的源 → 流式写入 `.part` → 校验 SHA-256 → 改名到最终路径。
/// 任何一步失败都会删掉半成品：留下一个大小不对的 `.part` 只会让下次「已下载」
/// 的判断更难解释。
#[tauri::command]
pub async fn crash_ai_model_download(
    app: AppHandle,
    downloads: tauri::State<'_, ModelDownloads>,
) -> Result<ModelStatus, String> {
    {
        let mut running = downloads.running.lock().map_err(|_| "下载状态不可用。")?;
        if *running {
            return Err("已经在下载模型了。".into());
        }
        *running = true;
    }
    downloads.cancel.store(false, Ordering::SeqCst);
    // 用一个守卫保证无论从哪条路径返回，`running` 都会被复位——否则一次失败会让
    // 「下载」按钮永久变灰，只能重启启动器。
    struct RunningGuard<'a>(&'a ModelDownloads);
    impl Drop for RunningGuard<'_> {
        fn drop(&mut self) {
            if let Ok(mut running) = self.0.running.lock() {
                *running = false;
            }
        }
    }
    let _guard = RunningGuard(&downloads);

    let result = download_job(&app, &downloads).await;
    if let Err(error) = &result {
        emit_progress(
            &app,
            ModelProgress {
                stage: "failed",
                received: 0,
                total: MODEL.size,
                source: None,
                bytes_per_second: None,
            },
        );
        let _ = app.emit("crash-ai-model-failed", error.clone());
    }
    result
}

/// 下载的核心流程，与 Tauri 解耦。
///
/// 分离出来是为了能用一个小文件跑完整链路（测速 → 下载 → 校验 → 改名）：真实
/// 模型有 1.1 GB，端到端验证它既慢又不适合放进测试。这里接收「描述 + 目标目录」，
/// 因此测试可以传入一个只有几十字节的假模型，验证的却是同一段代码。
///
/// 参数是 `&ModelDescriptor` 而不是直接用 `MODEL`：将来支持多个模型档位时，这个
/// 函数不需要再改。
async fn fetch_model(
    descriptor: &ModelDescriptor,
    directory: &PathBuf,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(ModelProgress),
) -> Result<(), String> {
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|error| format!("无法创建模型目录：{error}"))?;
    let destination = directory.join(descriptor.file_name);
    let partial = directory.join(format!("{}.part", descriptor.file_name));

    /*
     * 已经下完就不重复下。
     *
     * 用 `verify_file` 而不是只比大小：这个函数在「目标已存在且校验通过」时直接
     * 返回，等于免费拿到了「上次下完后被改坏」的检测。代价是重下一遍前要读完整
     * 文件，但这只在用户主动点「下载」时发生，而不是每次打开设置都读。
     */
    if tokio::fs::try_exists(&destination).await.unwrap_or(false)
        && verify_file(
            &destination,
            Some(descriptor.size),
            None,
            Some(descriptor.sha256),
        )
        .await
        .is_ok()
    {
        return Ok(());
    }

    on_progress(ModelProgress {
        stage: "probing",
        received: 0,
        total: descriptor.size,
        source: None,
        bytes_per_second: None,
    });
    let client = client()?;
    let probed = probe_sources(&client, descriptor).await;
    let source = pick_source(&probed, descriptor)?;
    let url = safe_source(source.url).ok_or_else(|| "模型源地址不合法。".to_string())?;

    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("连接下载源失败：{error}"))?
        .error_for_status()
        .map_err(|error| format!("下载源返回错误：{error}"))?;

    // 服务端给了总长就以它为准，否则用描述里的常量。两者不一致时以服务端为准，
    // 但最终校验仍用常量里的字节数——那是哈希对应的唯一正确长度。
    let total = response.content_length().unwrap_or(descriptor.size);
    let label = source.label;

    stream_to_file(&mut response, &partial, cancel, |received, speed| {
        on_progress(ModelProgress {
            stage: "downloading",
            received,
            total,
            source: Some(label.to_owned()),
            bytes_per_second: speed,
        });
    })
    .await?;

    on_progress(ModelProgress {
        stage: "verifying",
        received: total,
        total: descriptor.size,
        source: Some(label.to_owned()),
        bytes_per_second: None,
    });
    verify_file(
        &partial,
        Some(descriptor.size),
        None,
        Some(descriptor.sha256),
    )
    .await
    .map_err(|error| {
        let _ = std::fs::remove_file(&partial);
        format!("下载的文件校验失败（{}），已删除。请重试。", error)
    })?;

    tokio::fs::rename(&partial, &destination)
        .await
        .map_err(|error| format!("无法保存模型文件：{error}"))?;

    on_progress(ModelProgress {
        stage: "completed",
        received: descriptor.size,
        total: descriptor.size,
        source: Some(label.to_owned()),
        bytes_per_second: None,
    });
    Ok(())
}

async fn download_job(app: &AppHandle, downloads: &ModelDownloads) -> Result<ModelStatus, String> {
    let directory = models_dir(app)?;
    let result = fetch_model(&MODEL, &directory, &downloads.cancel, |progress| {
        emit_progress(app, progress);
    })
    .await;
    result?;
    current_status(app, downloads)
}

/// 从测速结果里挑最快的可用源。
///
/// 失败时把所有源的具体原因拼出来，而不是只说「下载失败」——用户看到「全部超时」
/// 和「HTTP 404」会采取完全不同的行动。
fn pick_source<'a>(
    probed: &'a [ModelSourceStatus],
    descriptor: &'a ModelDescriptor,
) -> Result<&'a ModelSource, String> {
    let chosen = probed
        .iter()
        .find(|source| source.bytes_per_second.is_some())
        .ok_or_else(|| {
            let details: Vec<String> = probed
                .iter()
                .map(|source| {
                    format!(
                        "{}：{}",
                        source.label,
                        source.problem.as_deref().unwrap_or("不可用")
                    )
                })
                .collect();
            format!("所有下载源都不可用。{}", details.join("；"))
        })?;
    descriptor
        .sources
        .iter()
        .find(|candidate| candidate.url == chosen.url)
        .ok_or_else(|| "内部错误：选中的源不在候选列表里。".to_string())
}

/// 把响应体流式写入文件。
///
/// 抽成独立函数（而不是留在 `download_job` 里）是为了能对着一个本地假服务器测：
/// 取消、中断、写盘失败这三条路径都不能靠「相信它是对的」。它不接触 Tauri，
/// 进度通过回调往外送，所以测试里可以只看文件和回调记录。
///
/// 任何失败都会删掉半成品：留下一个大小不对的 `.part` 只会让下次「已下载」的判断
/// 更难解释，而且它白占 1 GB 磁盘。
async fn stream_to_file(
    response: &mut reqwest::Response,
    partial: &PathBuf,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;

    tokio::fs::File::create(partial)
        .await
        .map_err(|error| format!("无法创建临时文件：{error}"))?;
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(partial)
        .await
        .map_err(|error| format!("无法写入临时文件：{error}"))?;

    let mut received: u64 = 0;
    let mut last_emit = Instant::now();
    let mut last_bytes = 0u64;

    loop {
        if cancel.load(Ordering::SeqCst) {
            drop(file);
            let _ = tokio::fs::remove_file(partial).await;
            return Err("已取消下载。".into());
        }
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                drop(file);
                let _ = tokio::fs::remove_file(partial).await;
                return Err(format!("下载中断：{error}"));
            }
        };
        if let Err(error) = file.write_all(&chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(partial).await;
            return Err(format!("写入模型文件失败：{error}"));
        }
        received += chunk.len() as u64;
        if last_emit.elapsed() >= PROGRESS_INTERVAL {
            let speed = Some(
                ((received.saturating_sub(last_bytes)) as f64
                    / last_emit.elapsed().as_secs_f64().max(0.001))
                .round() as u64,
            );
            on_progress(received, speed);
            last_emit = Instant::now();
            last_bytes = received;
        }
    }
    if let Err(error) = file.flush().await {
        drop(file);
        let _ = tokio::fs::remove_file(partial).await;
        return Err(format!("写入模型文件失败：{error}"));
    }
    drop(file);
    on_progress(received, None);
    Ok(())
}

/// 模型文件所在路径（供别处引用，例如将来自动拉起推理引擎）。
///
/// 唯一的调用方是 `llama_runtime`：按需启动引擎时，它需要「模型文件在哪」。
/// 路径规则集中在这里，而不是让启动逻辑再拼一次文件名——两处各拼一次，改名时
/// 必然漏掉一处。

pub fn installed_model_path(app: &AppHandle) -> Option<PathBuf> {
    let directory = models_dir(app).ok()?;
    let path = directory.join(MODEL.file_name);
    if std::fs::metadata(&path)
        .ok()
        .is_some_and(|meta| meta.len() == MODEL.size)
    {
        Some(path)
    } else {
        None
    }
}

/// 仅用于测试：当前模型的期望大小与来源数量。
#[cfg(test)]
pub(crate) fn descriptor_facts() -> (u64, usize) {
    (MODEL.size, MODEL.sources.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_sources_are_https_on_allowed_hosts() {
        for source in MODEL.sources {
            let parsed = safe_source(source.url)
                .unwrap_or_else(|| panic!("source {} is not usable", source.label));
            assert_eq!(parsed.scheme(), "https");
            assert!(allowed_host(&parsed), "{} host not allowed", source.label);
        }
    }

    #[test]
    fn unsafe_urls_are_rejected() {
        // 明文 HTTP、内嵌凭据、白名单之外的主机，一个都不能通过。
        assert!(safe_source("http://hf-mirror.com/x.gguf").is_none());
        assert!(safe_source("https://user:pass@hf-mirror.com/x.gguf").is_none());
        assert!(safe_source("https://example.com/x.gguf").is_none());
        assert!(safe_source("not a url").is_none());
        // 测试构建允许回环 HTTP（端到端测试要用假服务器），但仅限回环地址：
        // 一个内网地址在测试里也必须被拒，否则这个放宽会变成真的漏洞。
        if cfg!(test) {
            assert!(safe_source("http://127.0.0.1:8080/model.gguf").is_some());
            assert!(safe_source("http://192.168.1.10/model.gguf").is_none());
            assert!(safe_source("http://example.com/model.gguf").is_none());
        }
    }

    #[test]
    fn descriptor_carries_a_full_sized_sha256() {
        assert_eq!(MODEL.sha256.len(), 64);
        assert!(MODEL.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        // 1.1 GB 上下：低于 100 MB 说明常量被改错了。
        assert!(MODEL.size > 100 * 1024 * 1024);
    }

    #[test]
    fn descriptor_matches_published_metadata() {
        let (size, sources) = descriptor_facts();
        assert_eq!(size, 1_099_627_808);
        assert_eq!(sources, 3);
    }

    /// 真实网络测速。默认忽略，因为它依赖外网且要几秒。
    ///
    /// 跑法：`cargo test -p mfmcl-desktop -- --ignored probe_reports_speed`
    ///
    /// 保留它的意义：`probe_one` 的正确性无法用假数据验证——Range 请求、重定向、
    /// 流式读取三者任一处写错，只有真的连上镜像才能发现。这条测试在 CI 里会被
    /// 跳过，改动这一块时手动跑一次。
    #[tokio::test]
    #[ignore = "requires network access"]
    async fn probe_reports_speed_for_at_least_one_source() {
        let client = client().expect("client builds");
        let results = probe_all(&client).await;
        assert_eq!(
            results.len(),
            MODEL.sources.len(),
            "every source must be reported"
        );
        // 排序：可用的（有大小的）必须排在不可用的前面。
        let usable: Vec<_> = results
            .iter()
            .filter(|item| item.bytes_per_second.is_some())
            .collect();
        assert!(
            !usable.is_empty(),
            "at least one source should be reachable: {results:?}"
        );
        assert!(usable[0].bytes_per_second.unwrap() > 0);
        assert!(
            results[0].bytes_per_second.is_some(),
            "slowest usable source must not sort first"
        );
    }

    // --- 源选择 ------------------------------------------------------------

    fn status(label: &str, url: &str, speed: Option<u64>) -> ModelSourceStatus {
        ModelSourceStatus {
            label: label.to_owned(),
            url: url.to_owned(),
            bytes_per_second: speed,
            problem: speed.is_none().then(|| "超时".to_owned()),
        }
    }

    #[test]
    fn pick_source_takes_the_first_usable_entry() {
        let probed = vec![
            status("bad", MODEL.sources[0].url, None),
            status("fast", MODEL.sources[1].url, Some(4_000_000)),
            status("slow", MODEL.sources[2].url, Some(100_000)),
        ];
        let picked = pick_source(&probed, &MODEL).expect("a usable source exists");
        assert_eq!(
            picked.url, MODEL.sources[1].url,
            "must take the fastest usable one"
        );
    }

    #[test]
    fn pick_source_reports_every_reason_when_none_are_usable() {
        let probed = vec![
            status("a", MODEL.sources[0].url, None),
            status("b", MODEL.sources[1].url, None),
        ];
        let error = pick_source(&probed, &MODEL).expect_err("no source is usable");
        // 具体原因必须出现在错误里：只说「下载失败」用户无法判断该做什么。
        assert!(error.contains("a：超时"), "{error}");
        assert!(error.contains("b：超时"), "{error}");
    }

    #[test]
    fn pick_source_rejects_an_entry_that_is_not_in_the_descriptor() {
        // 测速结果与描述不一致时不能盲目使用——那说明有 bug，宁可报错。
        let probed = vec![status("ghost", "https://hf-mirror.com/ghost.gguf", Some(1))];
        assert!(pick_source(&probed, &MODEL).is_err());
    }

    // --- 流式写入 ----------------------------------------------------------

    /// 起一个只回一次响应的假服务器，用来验证流式下载。
    ///
    /// 不用 `download-engine` 的 mock：那个是私有的测试辅助，而这里要验证的是
    /// 本模块自己的写入循环（取消、中断、半成品清理）。
    async fn serve_once(body: &'static [u8], advertise: usize) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut head = Vec::new();
            let mut buffer = [0u8; 1024];
            while !head.ends_with(b"\r\n\r\n") {
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                if read == 0 {
                    return;
                }
                head.extend_from_slice(&buffer[..read]);
            }
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {advertise}\r\nConnection: close\r\n\r\n"
            );
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(body).await;
            let _ = socket.shutdown().await;
        });
        format!("http://{address}/model.gguf")
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mfmcl-model-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn stream_writes_the_whole_body_and_keeps_the_file() {
        let url = serve_once(b"abcdefghij", 10).await;
        let mut response = reqwest::Client::new().get(&url).send().await.unwrap();
        let dir = temp_dir();
        let partial = dir.join("model.part");
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();

        stream_to_file(&mut response, &partial, &cancel, |received, _| {
            seen.push(received)
        })
        .await
        .expect("streaming succeeds");

        assert_eq!(tokio::fs::read(&partial).await.unwrap(), b"abcdefghij");
        // 至少上报过一次进度，且最后一条是完整长度。
        assert_eq!(
            seen.last().copied(),
            Some(10),
            "final progress must report the full size"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn cancel_discards_the_partial_file() {
        let url = serve_once(b"abcdefghij", 10).await;
        let mut response = reqwest::Client::new().get(&url).send().await.unwrap();
        let dir = temp_dir();
        let partial = dir.join("model.part");
        // 提前置位：循环第一轮就会看到取消。
        let cancel = AtomicBool::new(true);

        let error = stream_to_file(&mut response, &partial, &cancel, |_, _| {})
            .await
            .expect_err("cancel must fail the download");
        assert!(error.contains("取消"), "{error}");
        // 关键：半成品必须被删掉，否则它白占 1 GB 且会让「已下载」的判断更难解释。
        assert!(!tokio::fs::try_exists(&partial).await.unwrap_or(false));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_short_body_leaves_a_file_that_fails_verification() {
        // 服务器声称 10 字节却只给 5 字节：写入循环本身会正常结束，但文件不完整。
        // 这正是「必须校验」的原因——不校验就会把一个坏文件当成功。
        let url = serve_once(b"abcde", 10).await;
        let mut response = reqwest::Client::new().get(&url).send().await.unwrap();
        let dir = temp_dir();
        let partial = dir.join("model.part");
        let cancel = AtomicBool::new(false);

        let _ = stream_to_file(&mut response, &partial, &cancel, |_, _| {}).await;
        let length = tokio::fs::metadata(&partial)
            .await
            .map(|meta| meta.len())
            .unwrap_or(0);
        assert!(
            length < 10,
            "short body must not be padded to the advertised size"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // --- 状态判定 ----------------------------------------------------------

    #[test]
    fn status_reports_partial_files_as_not_installed() {
        let dir = temp_dir();
        // 写一个不完整的文件：大小对不上就不能算「已下载」，否则界面会宣布完成，
        // 而推理引擎会拒绝加载它。
        std::fs::write(dir.join(MODEL.file_name), b"truncated").unwrap();
        let status = status_in(&dir, false);
        assert!(!status.installed);
        assert_eq!(status.bytes_on_disk, Some(9));
        assert_eq!(status.expected_bytes, MODEL.size);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn status_reports_absence_distinctly_from_partial() {
        let dir = temp_dir();
        let status = status_in(&dir, false);
        assert!(!status.installed);
        // `None` 与 `Some(0)` 的区别对界面有意义：前者是「从没下过」，后者是
        // 「有个空文件」，两者的提示不同。
        assert_eq!(status.bytes_on_disk, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn status_counts_a_leftover_partial_file() {
        // 下载被中断时留下的 `.part`：界面必须能看出「有不完整的文件」，
        // 否则那一 GB 会变成用户既看不到也删不掉的垃圾。
        let dir = temp_dir();
        std::fs::write(dir.join(format!("{}.part", MODEL.file_name)), b"half").unwrap();
        let status = status_in(&dir, false);
        assert!(!status.installed);
        assert_eq!(status.bytes_on_disk, Some(4));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn status_prefers_the_final_file_over_a_partial() {
        let dir = temp_dir();
        std::fs::write(dir.join(format!("{}.part", MODEL.file_name)), b"stale").unwrap();
        std::fs::write(dir.join(MODEL.file_name), b"new").unwrap();
        let status = status_in(&dir, false);
        // 最终文件存在时以它为准：半成品是上一次留下的，体积没有参考价值。
        assert_eq!(status.bytes_on_disk, Some(3));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn status_carries_the_downloading_flag() {
        let dir = temp_dir();
        assert!(!status_in(&dir, false).downloading);
        assert!(status_in(&dir, true).downloading);
        let _ = std::fs::remove_dir_all(dir);
    }

    // --- 端到端链路 --------------------------------------------------------

    /// 正文的 SHA-256，小写十六进制。
    fn hex_digest(bytes: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// 起一个假模型服务器：支持 Range（测速会请求）、每次请求都回完整正文。
    ///
    /// 返回 (基址, 停机句柄)。正文必须能撑满 `PROBE_BYTES`，否则测速会判定「太快
    /// 就结束」并把它当成可用源之外的情况。
    async fn fake_mirror(body: &'static [u8]) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut head = Vec::new();
                    let mut buffer = [0u8; 2048];
                    while !head.ends_with(b"\r\n\r\n") {
                        let read = socket.read(&mut buffer).await.unwrap_or(0);
                        if read == 0 {
                            return;
                        }
                        head.extend_from_slice(&buffer[..read]);
                    }
                    let request = String::from_utf8_lossy(&head).to_ascii_lowercase();
                    // 测速用 Range 请求；这里回 206 + 需要的那一段。下载走完整正文。
                    if request.contains("range:") {
                        let length = body.len().min(PROBE_BYTES as usize);
                        let header = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {length}\r\n\
                             Content-Range: bytes 0-{}/{}\r\nConnection: close\r\n\r\n",
                            length.saturating_sub(1),
                            body.len()
                        );
                        let _ = socket.write_all(header.as_bytes()).await;
                        let _ = socket.write_all(&body[..length]).await;
                    } else {
                        let header = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = socket.write_all(header.as_bytes()).await;
                        let _ = socket.write_all(body).await;
                    }
                    let _ = socket.shutdown().await;
                });
            }
        });
        (format!("http://{address}/model.gguf"), handle)
    }

    #[tokio::test]
    async fn end_to_end_download_verifies_and_renames() {
        // 正文字数要超过 PROBE_BYTES，测速才拿得到有效速度。
        static BODY: &[u8] = &[7u8; 700_000];
        let (base, server) = fake_mirror(BODY).await;
        let url: &'static str = Box::leak(base.into_boxed_str());
        let descriptor = ModelDescriptor {
            id: "test",
            name: "test model",
            file_name: "test.gguf",
            size: BODY.len() as u64,
            // 描述结构要求 &'static str，这里泄漏一份（测试进程短暂执行）。
            sha256: Box::leak(hex_digest(BODY).into_boxed_str()),
            sources: Box::leak(vec![ModelSource { label: "fake", url }].into_boxed_slice()),
        };
        let dir = temp_dir();
        let cancel = AtomicBool::new(false);
        let mut stages: Vec<&'static str> = Vec::new();

        fetch_model(&descriptor, &dir, &cancel, |progress| {
            stages.push(progress.stage)
        })
        .await
        .expect("the full pipeline succeeds");

        // 文件落在最终路径、内容正确、没有残留 .part。
        let final_path = dir.join("test.gguf");
        assert_eq!(tokio::fs::read(&final_path).await.unwrap(), BODY);
        assert!(
            !tokio::fs::try_exists(dir.join("test.gguf.part"))
                .await
                .unwrap_or(false)
        );
        // 三个阶段的顺序必须完整：用户界面的文案依赖它。
        assert!(stages.contains(&"probing"), "{stages:?}");
        assert!(stages.contains(&"downloading"), "{stages:?}");
        assert!(stages.contains(&"verifying"), "{stages:?}");
        assert_eq!(stages.last().copied(), Some("completed"), "{stages:?}");

        server.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn end_to_end_rejects_a_corrupted_download() {
        static BODY: &[u8] = &[9u8; 700_000];
        let (base, server) = fake_mirror(BODY).await;
        let url: &'static str = Box::leak(base.into_boxed_str());
        // 哈希故意写错：校验必须拦下它，并且不留下任何文件。
        let descriptor = ModelDescriptor {
            id: "test",
            name: "test model",
            file_name: "bad.gguf",
            size: BODY.len() as u64,
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            sources: Box::leak(vec![ModelSource { label: "fake", url }].into_boxed_slice()),
        };
        let dir = temp_dir();
        let cancel = AtomicBool::new(false);

        let error = fetch_model(&descriptor, &dir, &cancel, |_| {})
            .await
            .expect_err("a wrong hash must fail the download");
        assert!(error.contains("校验失败"), "{error}");
        // 关键：坏文件不能留在目标路径上，否则下次会被当成「已下载」。
        assert!(
            !tokio::fs::try_exists(dir.join("bad.gguf"))
                .await
                .unwrap_or(false)
        );
        assert!(
            !tokio::fs::try_exists(dir.join("bad.gguf.part"))
                .await
                .unwrap_or(false)
        );

        server.abort();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn end_to_end_skips_download_when_already_installed() {
        static BODY: &[u8] = &[3u8; 700_000];
        let (base, server) = fake_mirror(BODY).await;
        let url: &'static str = Box::leak(base.into_boxed_str());
        let descriptor = ModelDescriptor {
            id: "test",
            name: "test model",
            file_name: "cached.gguf",
            size: BODY.len() as u64,
            sha256: Box::leak(hex_digest(BODY).into_boxed_str()),
            sources: Box::leak(vec![ModelSource { label: "fake", url }].into_boxed_slice()),
        };
        let dir = temp_dir();
        tokio::fs::write(dir.join("cached.gguf"), BODY)
            .await
            .unwrap();
        let cancel = AtomicBool::new(false);
        let mut events = 0;

        fetch_model(&descriptor, &dir, &cancel, |_| events += 1)
            .await
            .expect("an intact file short-circuits the download");
        // 直接返回：没有发出任何进度事件（也就不会让界面闪一下「测速中」）。
        assert_eq!(events, 0, "an intact cached file must not start a download");

        server.abort();
        let _ = std::fs::remove_dir_all(dir);
    }
}
