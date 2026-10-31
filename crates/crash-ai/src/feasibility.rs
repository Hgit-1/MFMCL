// 判断这台机器能不能跑本地模型。
//
// ## 为什么需要单独判断
//
// 规则分析在本机几乎不耗资源，但本地模型是另一回事：1.7B 的 Q4 模型常驻要
// 1.1–1.5 GB 内存，推理时还要额外的 KV cache；CPU 推理约 26 tok/s，在更弱的
// 机器上会慢到几十秒甚至几分钟。对一台内存只有 2 GB 的机器开启模型，结果是
// 分析卡住、界面看起来死掉——而规则结论本来 4 毫秒就能给出。
//
// 所以这里回答一个独立的问题：**该不该启用模型**。它与「崩溃原因是什么」
// 无关，两者判断错的对象完全不同：
//
//   - `hardware::assess`   → 这台机器能不能跑这个**游戏**
//   - `feasibility::assess_llm` → 这台机器能不能跑这个**模型**
//
// 混在一起曾经造成过误解：用户说「带不动」时，指的是模型跑不动，而不是游戏。
//
// ## 两段式判断
//
// 1. **静态预估**（`assess_llm`）：分析前根据内存、核数、是否有 GPU 加速、
//    模型大小推断。这一步用来在**启动前**就给出提示，避免用户等一分钟才发现。
// 2. **运行时熔断**（`LlmHealth`）：连续失败或超时后自动停用，并把原因记下来。
//    静态预估无法预测所有情况（模型被别处加载、内存被别的程序占走），所以
//    真正可靠的依据是「试过之后失败了」。
//
// 两者都不阻断规则分析。模型是增强，不是前提。

use crate::hardware::Hardware;
use serde::{Deserialize, Serialize};

/// 模型能不能在这台机器上跑。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelFeasibility {
    /// 可以正常运行。
    Viable,
    /// 能跑，但会很慢（CPU 推理、核数偏少）。建议提示用户但它仍然值得开。
    Slow,
    /// 跑不动：内存不足或核数太低。应当停用模型并明确说明原因。
    NotViable,
    /// 无法判断（探测不到硬件信息）。
    Unknown,
}

/// 判断结果与依据。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAssessment {
    pub feasibility: ModelFeasibility,
    /// 一句话结论，直接显示给用户。
    pub summary: String,
    /// 依据：实测值 vs 需求。
    pub reason: String,
    /// 估算的常驻内存占用（MB），供界面显示。
    pub estimated_mb: u32,
    /// 建议：是否应该关闭模型。
    pub disable_model: bool,
}

/// 模型常驻内存的粗略估算（MB）。
///
/// 只用参数量与量化位数计算权重部分，再加 KV cache 与运行时开销。这不是精确值
/// ——不同后端的开销差别很大——但足以判断「2 GB 的机器能不能装下」。
///
/// 经验系数：
///   - Q4_K_M 约 4.5 bit/参数（含量化元数据），所以 1.7B ≈ 0.96 GB 权重；
///   - KV cache 与上下文长度成正比，8k 上下文对 1.7B 约 200–400 MB；
///   - 运行时（llama.cpp + 缓冲）约 300 MB。
pub fn estimate_llm_memory_mb(parameters_b: f32, context_tokens: u32) -> u32 {
    // 4.5 bit/参数 → 字节。
    let weights_mb = parameters_b * 1e9 * 4.5 / 8.0 / (1024.0 * 1024.0);
    // KV cache：经验上每 1000 token 对 1.7B 约 35 MB，按参数量线性缩放。
    let kv_mb = (context_tokens as f32 / 1000.0) * 35.0 * (parameters_b / 1.7).max(0.5);
    let runtime_mb = 300.0;
    (weights_mb + kv_mb + runtime_mb).round() as u32
}

/// 静态判断：这台机器跑不跑得动这个模型。
///
/// `parameters_b` 与 `context_tokens` 来自部署配置，不是用户输入——用户不该
/// 需要知道这些数字。
pub fn assess_llm(hardware: &Hardware, parameters_b: f32, context_tokens: u32) -> ModelAssessment {
    let estimated_mb = estimate_llm_memory_mb(parameters_b, context_tokens);

    let Some(total) = hardware.total_memory_mb else {
        /*
         * 探测不到内存时不要武断停用。
         *
         * 某些容器与受限环境读不到真实内存，此时「未知」比「不能用」安全：
         * 猜错方向的代价是让一台本来能跑的机器失去功能。
         */
        return ModelAssessment {
            feasibility: ModelFeasibility::Unknown,
            summary: "无法读取本机内存，未判断模型是否可用".into(),
            reason: "硬件探测没有返回内存信息".into(),
            estimated_mb,
            disable_model: false,
        };
    };

    /*
     * 模型之外还要留出运行空间：操作系统、启动器本身、以及分析时读入的日志。
     *
     * 用「可用内存」而不是「总内存」比较：模型是在浏览器/游戏开着的时候跑的，
     * 那时候剩下的才是真实可用的。可用值读不到时退回总内存的 70%。
     */
    let usable = hardware.available_memory_mb.unwrap_or((total as f64 * 0.7) as u32);

    // 1. 内存明显不够：可用内存不到估算需求 → 跑不动。
    if usable < estimated_mb {
        return ModelAssessment {
            feasibility: ModelFeasibility::NotViable,
            summary: format!(
                "本地模型需要约 {:.1} GB 内存，这台电脑当前只有约 {:.1} GB 可用",
                estimated_mb as f64 / 1024.0,
                usable as f64 / 1024.0
            ),
            reason: format!(
                "模型权重、KV cache 与运行时合计约 {estimated_mb} MB；\
                 当前可用内存 {usable} MB。加载会失败或触发系统换页。"
            ),
            estimated_mb,
            disable_model: true,
        };
    }

    // 2. 就算勉强装下，也没有余量：运行中随时可能被系统换页拖死。
    if usable < estimated_mb + 512 {
        return ModelAssessment {
            feasibility: ModelFeasibility::NotViable,
            summary: format!(
                "内存不足以同时容纳本地模型与系统本身（需要约 {:.1} GB，可用约 {:.1} GB）",
                (estimated_mb + 512) as f64 / 1024.0,
                usable as f64 / 1024.0
            ),
            reason: "模型占满可用内存后，系统换页会让推理慢到不可用。".into(),
            estimated_mb,
            disable_model: true,
        };
    }

    // 3. 核数太少：推理会慢到用户以为卡死。
    let cores = hardware.cpu_cores.unwrap_or(4);
    if cores <= 2 {
        return ModelAssessment {
            feasibility: ModelFeasibility::NotViable,
            summary: format!(
                "这台电脑只有 {cores} 个 CPU 核心，本地模型推理会慢到不可用"
            ),
            reason: format!(
                "1.7B 模型在 2 核上约 3–6 token/秒，一段解释要几分钟；\
                 规则分析本身只要几毫秒。"
            ),
            estimated_mb,
            disable_model: true,
        };
    }

    // 4. 能跑但慢：CPU 推理且核数不多。
    let has_gpu = hardware.gpu.is_some() && hardware.software_renderer != Some(true);
    if !has_gpu || cores <= 4 {
        return ModelAssessment {
            feasibility: ModelFeasibility::Slow,
            summary: "本地模型可以运行，但在 CPU 上每次分析需要十几秒".into(),
            reason: format!(
                "{cores} 个核心{}；1.7B 模型在 CPU 上约 20–30 token/秒。",
                if has_gpu { "" } else { "，未检测到可用的 GPU 加速" }
            ),
            estimated_mb,
            disable_model: false,
        };
    }

    ModelAssessment {
        feasibility: ModelFeasibility::Viable,
        summary: "本地模型可以正常运行".into(),
        reason: format!("{} 核，约 {:.1} GB 可用内存。", cores, usable as f64 / 1024.0),
        estimated_mb,
        disable_model: false,
    }
}

/// 运行时健康状态：连续失败后自动停用模型。
///
/// ## 为什么静态判断不够
///
/// 静态预估只能看内存与核数。真实的失败还有别的来源：端点填错、模型没加载、
/// 另一个进程把内存占走、请求被系统杀掉。这些只有「试过才知道」。
///
/// 熔断的意义是把「每次分析都等 90 秒超时」变成「第一次超时后就不再用」。
/// 没有它，一台跑不动模型的机器上每次点分析都要干等一分半，而规则结论本来
/// 立即可得。
#[derive(Debug, Default, Clone)]
pub struct LlmHealth {
    /// 连续失败次数。
    consecutive_failures: u32,
    /// 熔断后记录的原因，供界面显示。
    disabled_reason: Option<String>,
}

/// 连续失败多少次后停用。
///
/// 2 次而不是 1 次：一次失败可能是端点刚启动、模型正在加载，重试一次往往就好。
/// 但两次都失败就说明不是偶发问题。
const FAILURE_THRESHOLD: u32 = 2;

impl LlmHealth {
    pub fn new() -> Self {
        Self::default()
    }

    /// 模型当前是否应当被使用。
    pub fn is_available(&self) -> bool {
        self.disabled_reason.is_none()
    }

    /// 熔断原因，供界面显示。
    pub fn disabled_reason(&self) -> Option<&str> {
        self.disabled_reason.as_deref()
    }

    /// 记录一次成功，重置计数。
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
    }

    /// 记录一次失败，达到阈值时熔断。
    ///
    /// 返回 `true` 表示本次调用触发了熔断（用于只提示一次）。
    pub fn record_failure(&mut self, reason: &str) -> bool {
        if self.disabled_reason.is_some() {
            return false;
        }
        self.consecutive_failures += 1;
        if self.consecutive_failures >= FAILURE_THRESHOLD {
            self.disabled_reason = Some(format!(
                "本地模型连续 {FAILURE_THRESHOLD} 次不可用，已自动停用：{reason}"
            ));
            return true;
        }
        false
    }

    /// 用户手动重新启用，或服务重启后重置。
    pub fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.disabled_reason = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hardware(total: u32, available: u32, cores: u32, gpu: Option<&str>) -> Hardware {
        Hardware {
            total_memory_mb: Some(total),
            available_memory_mb: Some(available),
            cpu_cores: Some(cores),
            gpu: gpu.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn a_two_gigabyte_machine_cannot_run_the_model() {
        // 1.7B / 8k 约 1.5 GB；2 GB 机器可用也许 1 GB。
        let assessment = assess_llm(&hardware(2048, 1024, 4, None), 1.7, 8192);
        assert_eq!(assessment.feasibility, ModelFeasibility::NotViable);
        assert!(assessment.disable_model);
        // 结论必须说明是**模型**的内存，不是游戏。
        assert!(assessment.summary.contains("模型"));
        assert!(assessment.summary.contains("内存"));
    }

    #[test]
    fn enough_memory_but_no_headroom_is_still_not_viable() {
        // 可用刚过估算值，但没有余量：运行中会被换页拖死。
        let estimate = estimate_llm_memory_mb(1.7, 8192);
        let assessment = assess_llm(&hardware(8192, estimate + 100, 8, None), 1.7, 8192);
        assert_eq!(assessment.feasibility, ModelFeasibility::NotViable);
        assert!(assessment.reason.contains("换页"));
    }

    #[test]
    fn a_single_core_cannot_run_inference() {
        let assessment = assess_llm(&hardware(16384, 12288, 1, None), 1.7, 8192);
        assert_eq!(assessment.feasibility, ModelFeasibility::NotViable);
        assert!(assessment.summary.contains("核心"));
    }

    #[test]
    fn a_modest_machine_runs_but_should_warn_about_speed() {
        // 8 GB / 4 核 / 无 GPU：能跑，但该告诉用户要等。
        let assessment = assess_llm(&hardware(8192, 6144, 4, None), 1.7, 8192);
        assert_eq!(assessment.feasibility, ModelFeasibility::Slow);
        assert!(!assessment.disable_model, "slow is not a reason to disable");
        assert!(assessment.summary.contains("十几秒") || assessment.summary.contains("CPU"));
    }

    #[test]
    fn a_strong_machine_is_viable() {
        let assessment = assess_llm(&hardware(32768, 24576, 16, Some("NVIDIA RTX 4090")), 1.7, 8192);
        assert_eq!(assessment.feasibility, ModelFeasibility::Viable);
        assert!(!assessment.disable_model);
    }

    #[test]
    fn software_rendering_counts_as_no_gpu_acceleration() {
        let mut hw = hardware(32768, 24576, 8, Some("llvmpipe"));
        hw.software_renderer = Some(true);
        let assessment = assess_llm(&hw, 1.7, 8192);
        // 没有真正的 GPU 加速 → 归类为慢，而不是可行。
        assert_eq!(assessment.feasibility, ModelFeasibility::Slow);
    }

    #[test]
    fn missing_memory_data_does_not_disable_the_model() {
        /*
         * 探测失败时不能武断停用：容器里常常读不到内存，猜错的代价是让一台
         * 本来能跑的机器失去功能。
         */
        let assessment = assess_llm(&Hardware::default(), 1.7, 8192);
        assert_eq!(assessment.feasibility, ModelFeasibility::Unknown);
        assert!(!assessment.disable_model);
    }

    #[test]
    fn memory_estimate_grows_with_size_and_context() {
        assert!(estimate_llm_memory_mb(7.0, 8192) > estimate_llm_memory_mb(1.7, 8192));
        assert!(estimate_llm_memory_mb(1.7, 32768) > estimate_llm_memory_mb(1.7, 8192));
        // 1.7B / 8k 的估算应落在合理区间（实测常驻约 1.3–1.6 GB）。
        let estimate = estimate_llm_memory_mb(1.7, 8192);
        assert!((1200..1800).contains(&estimate), "got {estimate}");
    }

    #[test]
    fn circuit_breaker_trips_after_two_failures() {
        let mut health = LlmHealth::new();
        assert!(health.is_available());
        assert!(!health.record_failure("连接被拒绝"));
        assert!(health.is_available(), "one failure may be a cold start");
        assert!(health.record_failure("连接被拒绝"), "second failure trips it");
        assert!(!health.is_available());
        assert!(health.disabled_reason().unwrap().contains("自动停用"));
    }

    #[test]
    fn a_success_resets_the_failure_count() {
        let mut health = LlmHealth::new();
        health.record_failure("超时");
        health.record_success();
        // 计数已重置，再失败一次不该熔断。
        assert!(!health.record_failure("超时"));
        assert!(health.is_available());
    }

    #[test]
    fn a_tripped_breaker_reports_only_once() {
        let mut health = LlmHealth::new();
        health.record_failure("a");
        health.record_failure("b");
        // 已经熔断后再失败不该再报一次「刚停用」。
        assert!(!health.record_failure("c"));
    }

    #[test]
    fn manual_reset_re_enables_the_model() {
        let mut health = LlmHealth::new();
        health.record_failure("a");
        health.record_failure("b");
        assert!(!health.is_available());
        health.reset();
        assert!(health.is_available());
        assert!(health.disabled_reason().is_none());
    }
}
