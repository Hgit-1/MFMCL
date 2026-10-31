// Hardware capability assessment.
//
// ## Why this exists
//
// Some crashes are not crashes at all: the machine cannot run this game. Telling
// someone to "check their mods" when they have 2 GB of RAM and a 2009 GPU wastes
// their time and, worse, looks like the analyser does not understand the log.
//
// But declaring "your computer cannot run this" is a strong claim, and it is easy
// to be wrong about. A laptop with 8 GB can run a modded 1.12 pack fine; the same
// machine cannot run a 300-mod 1.21 pack with shaders. So the assessment is
// **relative to what is being launched**, and the verdict is only "hopeless" when
// the gap is not closable by turning settings down.
//
// ## Why the thresholds are coarse
//
// A 1.7B model, or any rule table, cannot model real performance. What it can do
// is catch the cases that are not close: 2 GB of RAM against a 300-mod pack,
// no hardware OpenGL, a CPU without SSE2. Those are unambiguous. Everything in
// between is reported as "tight" or "adequate" and left to the user — pretending
// to grade a machine precisely would be worse than saying nothing.
//
// ## Where the numbers come from
//
// They are deliberately conservative and derived from what Minecraft actually
// does, not from a benchmark:
//
//   - Vanilla 1.16+ needs ~1.5-2 GB to be playable, plus the JVM's own overhead.
//   - Each mod costs roughly 5-15 MB of resident classes plus its own data; the
//     estimator uses 8 MB because it is the middle of that range and errors on
//     the side of "you need more", which matches what players report.
//   - OpenGL 3.2 is the minimum for 1.17+ shaders; 4.5 for modern Sodium/Oculus.
//
// The numbers are deliberately *not* tuned to this machine's specs.

use serde::{Deserialize, Serialize};

/// What we managed to learn about the machine.
///
/// Every field is optional. Detection fails partially all the time — a headless
/// container has no GPU, a containerised process sees host RAM, a remote session
/// hides the real adapter. Missing data must never turn into a "hopeless" verdict.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hardware {
    pub total_memory_mb: Option<u32>,
    pub available_memory_mb: Option<u32>,
    pub cpu_cores: Option<u32>,
    /// Lowest reported CPU clock in MHz, when the platform exposes it.
    #[serde(default)]
    pub cpu_mhz: Option<u32>,
    /// Architecture string, e.g. `x86_64`.
    #[serde(default)]
    pub arch: Option<String>,
    /// Free disk space on the partition holding the game, in MB.
    #[serde(default)]
    pub free_disk_mb: Option<u64>,
    /// Graphics adapter, when it could be read.
    #[serde(default)]
    pub gpu: Option<String>,
    /// Whether the session has a display (`DISPLAY` / `WAYLAND_DISPLAY` on Linux).
    /// A game cannot start without one, and that is a very different problem
    /// from "your GPU is too old".
    #[serde(default)]
    pub has_display: Option<bool>,
    /// Software renderer detected (llvmpipe / swiftshader / Microsoft Basic).
    #[serde(default)]
    pub software_renderer: Option<bool>,
}

/// What is being launched. The verdict is meaningless without this.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workload {
    #[serde(default)]
    pub game_version: Option<String>,
    #[serde(default)]
    pub loader: Option<String>,
    #[serde(default)]
    pub mod_count: Option<usize>,
    /// Memory the launcher is configured to give the game.
    #[serde(default)]
    pub allocated_memory_mb: Option<u32>,
}

/// How well the machine matches the workload.
///
/// ## Why the ordering is explicit rather than derived
///
/// The derive order (`Tight` < `Adequate` < `Ample` < `Hopeless`) is *not* a
/// severity order, and using `max()` on it was a real bug: a machine with plenty
/// of RAM but two cores came out as `Ample` because `Ample > Tight` numerically,
/// so the only warning was ignored. `severity()` exists to make "which verdict
/// wins" explicit and impossible to get wrong by reordering the variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Comfortably enough.
    Ample,
    /// Enough.
    Adequate,
    /// Not enough, but recoverable by turning settings down.
    Tight,
    /// The gap is not closable by settings. This is the "彻底带不动" case.
    Hopeless,
}

impl Verdict {
    /// Higher is worse. Used to pick the verdict that describes the whole machine.
    pub fn severity(self) -> u8 {
        match self {
            Self::Ample => 0,
            Self::Adequate => 1,
            Self::Tight => 2,
            Self::Hopeless => 3,
        }
    }

    /// The verdict describing a set of per-resource verdicts: the worst one.
    pub fn worst(values: impl IntoIterator<Item = Verdict>) -> Verdict {
        values
            .into_iter()
            .max_by_key(|verdict| verdict.severity())
            .unwrap_or(Verdict::Adequate)
    }
}

/// One concrete resource judgement.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assessment {
    /// `memory` / `cpu` / `gpu` / `disk` / `display`.
    pub resource: String,
    pub verdict: Verdict,
    /// What we measured, in the resource's own units, as a display string.
    pub measured: String,
    /// What this workload needs.
    pub required: String,
    pub note: String,
}

/// The whole picture.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    /// The worst verdict across all resources.
    pub verdict: Verdict,
    pub assessments: Vec<Assessment>,
    /// Suggestions that are about the machine, not about the crash.
    #[serde(default)]
    pub advice: Vec<String>,
    /// True only when a resource check could actually be run. An empty result
    /// means "we do not know", and the UI must not display it as a pass.
    pub detected: bool,
}

impl Capability {
    pub fn unknown() -> Self {
        Self {
            verdict: Verdict::Adequate,
            assessments: Vec::new(),
            advice: Vec::new(),
            detected: false,
        }
    }
}

/// Minimum RAM for a playable experience, given how much is being loaded.
///
/// Base is the game plus the JVM; mods add on top. Deliberately on the generous
/// side: telling someone their machine is fine when it is not leads to a crash
/// they will report to a mod author by mistake.
pub fn required_memory_mb(workload: &Workload) -> u32 {
    let base = match workload.game_version.as_deref() {
        // Legacy versions genuinely run in a few hundred MB.
        Some(version) if version.starts_with("1.7") || version.starts_with("1.8") => 1024,
        Some(version) if version.starts_with("1.12") => 1536,
        // 1.17+ moved rendering to a much heavier pipeline.
        Some(version) if version.starts_with("1.17") || version.starts_with("1.18") => 2048,
        _ => 2048,
    };
    let mods = workload.mod_count.unwrap_or(0) as u32;
    /*
     * 8 MB per mod is the middle of the observed 5-15 MB range. Capped so a
     * 500-mod pack does not produce an absurd requirement — past a point the pack
     * is unplayable for reasons other than RAM, and demanding 6 GB would make
     * the assessment useless on any normal machine.
     */
    let mod_allowance = (mods * 8).min(4096);
    base + mod_allowance
}

/// Assess the machine against the workload.
///
/// `hardware.total_memory_mb` is system RAM, `workload.allocated_memory_mb` is what
/// the JVM is told to take. Both matter: a machine with 64 GB that allocates 512 MB
/// will still OOM, and that is a settings problem rather than a hardware one.
pub fn assess(hardware: &Hardware, workload: &Workload) -> Capability {
    let mut assessments = Vec::new();
    let mut advice = Vec::new();
    let mut detected = false;

    // --- Memory ------------------------------------------------------------
    if let Some(total) = hardware.total_memory_mb {
        detected = true;
        let required = required_memory_mb(workload);
        /*
         * Compare against total RAM, not free RAM.
         *
         * Free RAM at analysis time is mostly irrelevant: the user will close the
         * browser before playing. Total is the honest ceiling.
         */
        let verdict = if total < 2048 {
            // Under 2 GB cannot run any modern Minecraft at all.
            Verdict::Hopeless
        } else if total < required {
            if total * 2 < required {
                // Less than half of what is needed: no setting closes this gap.
                Verdict::Hopeless
            } else {
                Verdict::Tight
            }
        } else if total >= required.saturating_mul(2) {
            Verdict::Ample
        } else {
            Verdict::Adequate
        };

        assessments.push(Assessment {
            resource: "memory".into(),
            verdict,
            measured: format_gb(total),
            required: format_gb(required),
            note: memory_note(verdict, workload),
        });

        /*
         * 「彻底带不动」的建议必须**不留余地**。
         *
         * 对一台只有一半内存的机器说「可以尝试减少模组」是误导：它会让用户以为
         * 删几个模组就能跑，实际是反复失败。这类机器该听到的是「这个版本不适合
         * 这台电脑」。所以两种 hopeless 的措辞分开写。
         */
        if verdict == Verdict::Hopeless {
            if total < 2048 {
                advice.push(format!(
                    "这台电脑只有 {} 内存。现代 Minecraft 需要至少 2 GB 才能启动，\
                     这不是靠设置能解决的——请换一台内存更大的电脑，或使用更旧的版本。",
                    format_gb(total)
                ));
            } else {
                advice.push(format!(
                    "这台电脑有 {} 内存，而这个整合包（{} 个模组）至少需要 {}，\
                     相差超过一半。减少模组或许能启动，但这个整合包本身不适合这台电脑；\
                     更现实的选择是换一个更轻的整合包。",
                    format_gb(total),
                    workload.mod_count.unwrap_or(0),
                    format_gb(required)
                ));
            }
        }
    }

    // --- Allocated heap ----------------------------------------------------
    if let (Some(allocated), Some(total)) = (workload.allocated_memory_mb, hardware.total_memory_mb) {
        detected = true;
        /*
         * The JVM cannot take essentially all of the machine: the OS, the launcher
         * and the JVM's own non-heap memory need room. Above ~85% of total RAM the
         * allocation is itself a likely cause of the crash, which is exactly the
         * "Could not reserve enough space" case.
         */
        let ceiling = (total as f64 * 0.85) as u32;
        if allocated > ceiling {
            assessments.push(Assessment {
                resource: "heap".into(),
                /*
                 * 超配堆是**设置问题**，不是「电脑太老」。
                 * 判定为 Tight 而不是 Hopeless：调低一个数字就能解决，把它说成
                 * 「带不动」会让用户以为该换电脑。
                 */
                verdict: Verdict::Tight,
                measured: format_gb(allocated),
                required: format!("≤ {}", format_gb(ceiling)),
                note: format!(
                    "分配的内存（{}）超过了这台电脑能安全给 JVM 的上限（约 {}）。\
                     系统本身、启动器和 JVM 的非堆内存都要占地方。",
                    format_gb(allocated),
                    format_gb(ceiling)
                ),
            });
            advice.push(format!(
                "把内存分配降到 {} 左右（当前是 {}）",
                format_gb(ceiling),
                format_gb(allocated)
            ));
        }
    }

    // --- CPU ---------------------------------------------------------------
    if let Some(cores) = hardware.cpu_cores {
        detected = true;
        /*
         * Two cores is the floor for a modded pack that spawns worker threads;
         * one core is not a performance problem but a "this will take minutes to
         * load, if it loads" problem.
         */
        let verdict = if cores <= 1 {
            Verdict::Hopeless
        } else if cores == 2 {
            Verdict::Tight
        } else if cores >= 8 {
            Verdict::Ample
        } else {
            Verdict::Adequate
        };
        assessments.push(Assessment {
            resource: "cpu".into(),
            verdict,
            measured: format!("{cores} 核"),
            required: "≥ 4 核".into(),
            note: match verdict {
                Verdict::Hopeless => "单核 CPU 无法在合理时间内加载现代版本。".into(),
                Verdict::Tight => "双核可以启动，但加载和世界生成会明显偏慢。".into(),
                _ => String::new(),
            },
        });
        if verdict == Verdict::Hopeless {
            advice.push("这台电脑的 CPU 只有 1 个核心，无法运行这个版本".into());
        }
    }

    // --- Display -----------------------------------------------------------
    if let Some(has_display) = hardware.has_display {
        detected = true;
        if !has_display {
            /*
             * Not a hardware verdict: the machine may be perfectly capable and
             * simply have no GUI session (SSH, container, systemd service). Kept
             * separate from "too weak" on purpose — the fix is completely different.
             */
            assessments.push(Assessment {
                resource: "display".into(),
                verdict: Verdict::Hopeless,
                measured: "没有图形会话".into(),
                required: "可用的桌面环境".into(),
                note: "当前环境没有图形显示（DISPLAY 未设置），游戏窗口无法创建。\
                       这通常意味着你在 SSH 或容器里运行，而不是电脑本身带不动。"
                    .into(),
            });
            advice.push("在正常的桌面会话中启动游戏".into());
        }
    }

    // --- Software rendering ------------------------------------------------
    if hardware.software_renderer == Some(true) {
        detected = true;
        assessments.push(Assessment {
            resource: "gpu".into(),
            verdict: Verdict::Tight,
            measured: "软件渲染".into(),
            required: "硬件加速的 OpenGL".into(),
            note: "当前使用的是软件渲染（如 llvmpipe），帧率会极低。通常是显卡驱动没装好。".into(),
        });
        advice.push("安装或修复显卡驱动，确保 Minecraft 使用硬件加速".into());
    }

    // --- Disk --------------------------------------------------------------
    if let Some(free) = hardware.free_disk_mb {
        detected = true;
        if free < 512 {
            assessments.push(Assessment {
                resource: "disk".into(),
                verdict: Verdict::Hopeless,
                measured: format_disk(free),
                required: "≥ 512 MB".into(),
                note: "磁盘空间不足，游戏无法写入存档或日志。".into(),
            });
            advice.push("清理磁盘空间后重试".into());
        }
    }

    let verdict = Verdict::worst(assessments.iter().map(|item| item.verdict));

    Capability {
        verdict,
        assessments,
        advice,
        detected,
    }
}

fn format_gb(mb: u32) -> String {
    if mb >= 1024 {
        let gb = mb as f64 / 1024.0;
        if (gb - gb.round()).abs() < 0.05 {
            format!("{} GB", gb.round() as u32)
        } else {
            format!("{gb:.1} GB")
        }
    } else {
        format!("{mb} MB")
    }
}

fn format_disk(mb: u64) -> String {
    if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

fn memory_note(verdict: Verdict, workload: &Workload) -> String {
    let mods = workload.mod_count.unwrap_or(0);
    match verdict {
        Verdict::Hopeless => format!(
            "物理内存低于运行这个版本（{mods} 个模组）所需的下限。\
             这不是靠调低画质能解决的——模组和游戏基础内存已经超过机器总量。"
        ),
        Verdict::Tight => format!(
            "内存勉强够启动，但{mods} 个模组加上区块加载很可能在游戏中耗尽。\
             建议减少模组或降低视距。"
        ),
        Verdict::Adequate => String::new(),
        Verdict::Ample => String::new(),
    }
}

// ---------------------------------------------------------------------------
// 自动探测
// ---------------------------------------------------------------------------

/// 探测本机硬件。
///
/// 为什么每次分析都重新探测而不是缓存：用户可能在分析过程中插了内存条、
/// 换了显卡驱动，或者（更常见）刚从「省电模式」切回来。探测本身只读几个文件
/// 与 sysinfo 的内存信息，代价远低于一次模型推理。
///
/// 所有字段都允许失败：容器里读不到 GPU、无头环境没有 DISPLAY、某些平台不暴露
/// 主频。任何一项拿不到就留 `None`，绝不用默认值冒充真实数据——那会让
/// 「带得动」的结论建立在编造的数字上。
pub fn detect() -> Hardware {
    /*
     * sysinfo 0.30 的 API：`refresh_memory` 与 `refresh_cpu` 分开调用；
     * `total_memory` 返回字节（`u64`）。
     */
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    system.refresh_cpu();

    let total_memory_mb = to_mb(system.total_memory());
    let available_memory_mb = to_mb(system.available_memory());

    let cpu_cores = system.cpus().len() as u32;
    let cpu_mhz = system
        .cpus()
        .iter()
        .map(|cpu| cpu.frequency())
        .filter(|mhz| *mhz > 0)
        .min()
        .map(|mhz| mhz as u32);

    Hardware {
        total_memory_mb: (total_memory_mb > 0).then_some(total_memory_mb),
        available_memory_mb: (available_memory_mb > 0).then_some(available_memory_mb),
        cpu_cores: (cpu_cores > 0).then_some(cpu_cores),
        cpu_mhz,
        arch: Some(std::env::consts::ARCH.to_owned()),
        free_disk_mb: None,
        gpu: detect_gpu(),
        has_display: detect_display(),
        software_renderer: detect_software_renderer(),
    }
}

fn to_mb(bytes: u64) -> u32 {
    (bytes / (1024 * 1024)).min(u32::MAX as u64) as u32
}

/// 显示器是否存在。
///
/// 只在 Linux 上判断：Windows 与 macOS 总是有窗口服务器，而 Linux 上 SSH 会话
/// 与容器里没有，那是一个与「性能不足」完全不同的失败原因。
#[cfg(target_os = "linux")]
fn detect_display() -> Option<bool> {
    let has = std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    Some(has)
}

#[cfg(not(target_os = "linux"))]
fn detect_display() -> Option<bool> {
    // 这两个平台总是有窗口服务器；不判断好过误判。
    None
}

/// 显卡名称。读不到就返回 `None`。
#[cfg(target_os = "linux")]
fn detect_gpu() -> Option<String> {
    /*
     * 从 `/sys/class/drm` 下读厂商与设备 ID 需要查表，代价大且容易过时。
     * `lspci` 输出里有可读名字，但它不保证存在。折中：先试 `lspci`，失败
     * 就算拿不到——「未知」比一个错误的型号更有用。
     */
    let output = std::process::Command::new("lspci").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find(|line| line.contains("VGA compatible controller") || line.contains("3D controller"))
        .map(|line| {
            line.split_once(": ")
                .map(|(_, rest)| rest.trim().to_owned())
                .unwrap_or_else(|| line.trim().to_owned())
        })
}

#[cfg(not(target_os = "linux"))]
fn detect_gpu() -> Option<String> {
    None
}

fn detect_software_renderer() -> Option<bool> {
    /*
     * llvmpipe / swiftshader / Microsoft Basic Render 都意味着「没有硬件加速」。
     * 这个信息很重要：一台配置不低的机器跑软件渲染，帧率会低到无法玩，而用户
     * 会以为是模组的问题。
     */
    let gpu = detect_gpu()?;
    let lower = gpu.to_ascii_lowercase();
    Some(
        lower.contains("llvmpipe")
            || lower.contains("swiftshader")
            || lower.contains("basic render")
            || lower.contains("software"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workload(mods: usize, version: &str, allocated: u32) -> Workload {
        Workload {
            game_version: Some(version.into()),
            mod_count: Some(mods),
            allocated_memory_mb: Some(allocated),
            ..Default::default()
        }
    }

    fn hw(total: u32, cores: u32) -> Hardware {
        Hardware {
            total_memory_mb: Some(total),
            cpu_cores: Some(cores),
            ..Default::default()
        }
    }

    #[test]
    fn an_old_machine_is_declared_hopeless() {
        // 2 GB RAM, 2 cores, a 300-mod 1.18 pack: not closable by settings.
        let assessment = assess(&hw(2048, 2), &workload(300, "1.18.2", 2048));
        assert_eq!(assessment.verdict, Verdict::Hopeless);
        assert!(assessment.advice.iter().any(|item| item.contains("内存")));
    }

    #[test]
    fn a_midrange_machine_running_the_same_pack_is_only_tight() {
        // 8 GB against a 300-mod pack needs 2+2.4 GB — enough to start.
        let assessment = assess(&hw(8192, 4), &workload(300, "1.18.2", 4096));
        assert!(
            assessment.verdict == Verdict::Tight || assessment.verdict == Verdict::Adequate,
            "got {:?}",
            assessment.verdict
        );
        assert_ne!(assessment.verdict, Verdict::Hopeless);
    }

    #[test]
    fn a_good_machine_is_ample() {
        let assessment = assess(&hw(32768, 16), &workload(20, "1.21.1", 4096));
        assert_eq!(assessment.verdict, Verdict::Ample);
    }

    #[test]
    fn single_core_is_hopeless_but_two_cores_is_not() {
        assert_eq!(assess(&hw(8192, 1), &workload(0, "1.21.1", 2048)).verdict, Verdict::Hopeless);
        assert_eq!(assess(&hw(8192, 2), &workload(0, "1.21.1", 2048)).verdict, Verdict::Tight);
    }

    #[test]
    fn missing_display_is_reported_separately_from_weak_hardware() {
        let hardware = Hardware {
            total_memory_mb: Some(32768),
            cpu_cores: Some(16),
            has_display: Some(false),
            ..Default::default()
        };
        let assessment = assess(&hardware, &workload(0, "1.21.1", 4096));
        let display = assessment
            .assessments
            .iter()
            .find(|item| item.resource == "display")
            .expect("display check must run");
        // The advice must point at the session, not at buying a new PC.
        assert!(display.note.contains("SSH") || display.note.contains("容器"));
    }

    #[test]
    fn overcommitted_heap_is_flagged_without_calling_the_machine_hopeless() {
        // 8 GB machine told to give the JVM 8 GB: a settings bug, not old hardware.
        let assessment = assess(&hw(8192, 8), &workload(10, "1.21.1", 8192));
        assert!(
            assessment.assessments.iter().any(|item| item.resource == "heap"),
            "heap overcommit must be reported"
        );
        assert_ne!(
            assessment.verdict,
            Verdict::Hopeless,
            "a settings mistake must not be reported as a dead machine"
        );
    }

    #[test]
    fn no_detected_data_never_claims_a_verdict() {
        let assessment = assess(&Hardware::default(), &workload(0, "1.21.1", 2048));
        assert!(!assessment.detected);
        assert!(assessment.assessments.is_empty());
    }

    #[test]
    fn requirement_scales_with_mod_count_and_version() {
        assert!(required_memory_mb(&workload(200, "1.21.1", 0)) > required_memory_mb(&workload(0, "1.21.1", 0)));
        assert!(required_memory_mb(&workload(0, "1.21.1", 0)) > required_memory_mb(&workload(0, "1.8.9", 0)));
    }
}
