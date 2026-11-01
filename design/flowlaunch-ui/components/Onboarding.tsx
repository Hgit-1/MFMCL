// 首次启动的引导配置。
//
// ## 它要解决的问题
//
// 旧版首次运行只有一屏「欢迎 + 开始使用」，点完之后用户面对的是一个空游戏库，
// 而且真正影响体验的四件事一件都还没定：界面语言、主题与强调色、给游戏分多少
// 内存、Java 在哪。这些默认值都能用，但对新机器往往不是最优——尤其是 Java，
// 没探测到就启动不了任何实例。
//
// 所以引导把这几项按「从最无关到最要紧」排列，用可回退的步骤问一遍，技术上等价
// 于把用户手动打开的设置面板逐页过一遍，但不需要他先知道这些设置在哪。
//
// ## 关键取舍
//
//   - **每一步都可跳过。** 引导不是门槛，用户可以一路「下一步」直接进主界面，
//     所有值保持默认。挡住新用户进游戏，比少配一项的代价大得多。
//   - **不引入新的存储。** 语言写 `flowlaunch-language`、主题写 `theme`、内存与
//     强调色写后端 `LauncherSettings`、Java 写 `flowlaunch-java-runtimes`——
//     全部复用设置面板已经在用的那套。引导只额外记一个「走完了」的时间戳
//     （见 `utils/onboarding.ts`）。
//   - **改动即时生效、不做暂存。** 选主题就立刻换肤，选语言就立刻换文案，探测
//     Java 就立刻入库。这样每一步的结果用户当场就能看到，点「开始使用」时不需要
//     再等一次批量保存——也就不会出现「保存失败但你已经在主界面了」的半成品状态。
//   - **模型那一步只提示、不自动下。** 它是一个 1.1 GB 的下载，必须由用户明确
//     点。引导里放一个按钮，跳过去由他自己决定，比在引导里替他决定更合适。
//
// ## 为什么不是对话框队列
//
// 五个独立的 Modal 会各自抢焦点、各自处理 Escape，返回上一步时状态也容易丢。
// 这里是一个整页视图，步骤之间只是内容替换，布局稳定，窄屏下也不会因为弹窗
// 高度上限而出现内部滚动条。

import { useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle,
  ArrowLeft,
  ArrowRight,
  Check,
  Cpu,
  Languages,
  MemoryStick,
  Palette,
  Sparkles,
  Terminal,
} from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import { LANGUAGES } from '../utils/i18n';
import { renderLocalized } from '../utils/i18n/localized';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import {
  MIN_MEMORY_MB,
  autoAllocateMb,
  deviceMemoryMbSync,
  lastKnownUsedMb,
  maxSliderIndexForTotal,
  MIN_SLIDER_INDEX,
  gbToSliderIndex,
  sliderIndexToGb,
} from '../utils/memory';
import { detectJavaRuntimes, type DetectResult } from '../utils/javaBridge';
import { saveJavaList, type JavaRuntime } from '../utils/java';
import { formatBytes, getCrashAiStatus, getModelStatus, isDesktopShell, type CrashModelAssessment, type ModelStatus } from '../utils/native';
import type { LauncherPrefs } from '../utils/launcherPrefs';
import styles from './Onboarding.module.css';

/** 强调色预设。与设置面板里的主题预设是同一组值，避免两处颜色不一致。 */
const PRESETS: { id: string; labelKey: MessageKey; color: string }[] = [
  { id: 'ocean', labelKey: 'settings.presetOcean', color: '#53b4f6' },
  { id: 'moss', labelKey: 'settings.presetMoss', color: '#41c98b' },
  { id: 'lavender', labelKey: 'settings.presetLavender', color: '#8b7cff' },
  { id: 'ember', labelKey: 'settings.presetEmber', color: '#f19b52' },
];

const THEME_KEYS: Record<'light' | 'dark' | 'system', MessageKey> = {
  light: 'settings.themeLight',
  dark: 'settings.themeDark',
  system: 'settings.themeSystem',
};

interface OnboardingProps {
  /** 当前的界面偏好。引导里的选择直接改它，走的是设置面板同一条保存链路。 */
  prefs: LauncherPrefs;
  /** 改一项偏好。父组件负责落库（后端设置或 localStorage）。 */
  onPrefsChange: (next: LauncherPrefs) => void;
  /**
   * 当前主题。
   *
   * 必须由父组件传入，不能再读 `prefs.theme`：主题真正生效的地方是 `useTheme`
   * 的 `theme` 状态（localStorage 的 `theme` 键），而 `prefs.theme` 是后端设置
   * 的一份快照，默认值还是 `'dark'`。两者不同源，于是出现了「实际跟随系统、
   * 切换器却高亮深色，点浅色后页码变亮但高亮仍停在深色」的错位。
   */
  theme: 'light' | 'dark' | 'system';
  onThemeChange: (theme: 'light' | 'dark' | 'system') => void;
  /** 强调色即时预览：切到下一步时用户应当已经看到新配色。 */
  onAccentChange: (color: string) => void;
  /** 已探测到的 Java 运行时。 */
  runtimes: JavaRuntime[];
  onRuntimesChange: (runtimes: JavaRuntime[]) => void;
  dark: boolean;
  /** 完成引导。父组件据此记录完成标记并进入主界面。 */
  onDone: () => void;
  /** 跳到「设置 → 下载」。模型那一步用它把「想下载就去这里」变成一次点击。 */
  onOpenModelSettings: () => void;
}

/** 步骤标识。顺序即呈现顺序。 */
type StepId = 'welcome' | 'language' | 'appearance' | 'memory' | 'java' | 'model';

const STEPS: StepId[] = ['welcome', 'language', 'appearance', 'memory', 'java', 'model'];

export function Onboarding({
  prefs,
  onPrefsChange,
  theme,
  onThemeChange,
  onAccentChange,
  runtimes,
  onRuntimesChange,
  dark,
  onDone,
  onOpenModelSettings,
}: OnboardingProps) {
  const { t, language, preference, setLanguage } = useI18n();
  const [step, setStep] = useState<StepId>('welcome');
  /** 焦点落在标题上：进入下一步时读屏会读出这一屏讲什么，而不是停在上一屏的按钮。 */
  const headingRef = useRef<HTMLHeadingElement>(null);

  // --- Java 探测 -----------------------------------------------------------
  const [javaBusy, setJavaBusy] = useState(false);
  const [javaNote, setJavaNote] = useState<DetectResult | null>(null);
  /**
   * 进入 Java 那一步时自动探测一次。
   *
   * 用「是否已经到过这一步」而不是「组件挂载」：在欢迎页就占着 CPU 扫盘没有必要，
   * 而用户真正看到这一屏时，结果应当已经在手边。只扫一次——重复扫会把用户手动
   * 调整过的顺序冲掉。
   */
  const probed = useRef(false);
  useEffect(() => {
    if (step !== 'java' || probed.current) return;
    probed.current = true;
    void runDetect();
  }, [step]);

  const runDetect = async () => {
    setJavaBusy(true);
    try {
      const result = await detectJavaRuntimes();
      setJavaNote(result);
      /*
       * 合并而不是覆盖：已有的条目保持原顺序。
       *
       * 后端按路径去重时用的是「列表顺序 = 优先级」，所以新发现的一律追加到末尾，
       * 由用户自己决定要不要提前——这与会话中在设置面板里点「探测」的语义一致。
       */
      const merged = [...runtimes, ...result.runtimes.filter(found => !runtimes.some(known => known.path === found.path))];
      onRuntimesChange(merged);
      saveJavaList(merged);
    } finally {
      setJavaBusy(false);
    }
  };

  // --- 内存 ---------------------------------------------------------------
  const totalGb = useMemo(() => Math.max(2, deviceMemoryMbSync() / 1024), []);
  const maxIndex = Math.max(MIN_SLIDER_INDEX + 1, maxSliderIndexForTotal(totalGb));
  const autoMb = useMemo(
    () => autoAllocateMb({ totalMb: totalGb * 1024, usedMb: lastKnownUsedMb() }),
    [totalGb],
  );
  const effectiveMb = prefs.memoryMode === 'auto' ? autoMb : prefs.memoryMb;
  const sliderValue = Math.min(maxIndex, Math.max(MIN_SLIDER_INDEX, gbToSliderIndex(effectiveMb / 1024)));

  // --- 模型 ---------------------------------------------------------------
  const [model, setModel] = useState<ModelStatus | null>(null);
  /**
   * 本机能不能跑这个模型。
   *
   * 与「有没有下载」是两个独立的问题，必须分开回答：下载了也可能跑不动（内存
   * 不足），没下载也未必跑不动。合成一句话会让用户走错方向——去下 2 GB 然后发现
   * 用不了，或者以为机器带不动而放弃一个其实能用的功能。
   */
  const [feasibility, setFeasibility] = useState<CrashModelAssessment | null>(null);
  useEffect(() => {
    if (step !== 'model') return;
    void getModelStatus().then(setModel).catch(() => setModel(null));
    void getCrashAiStatus()
      .then(status => setFeasibility(status?.feasibility ?? null))
      .catch(() => setFeasibility(null));
  }, [step]);

  const index = STEPS.indexOf(step);
  const isFirst = index === 0;
  const isLast = index === STEPS.length - 1;
  const goTo = (next: StepId) => setStep(next);
  const goNext = () => setStep(STEPS[Math.min(index + 1, STEPS.length - 1)]);
  const goBack = () => setStep(STEPS[Math.max(index - 1, 0)]);

  useEffect(() => {
    headingRef.current?.focus();
  }, [step]);

  /** 每一步的标题与说明。集中在一处，便于核对措辞是否形成一条连贯的路径。 */
  const headings: Record<StepId, { title: MessageKey; hint: MessageKey }> = {
    welcome: { title: 'onboarding.welcomeTitle', hint: 'onboarding.welcomeHint' },
    language: { title: 'onboarding.languageTitle', hint: 'onboarding.languageHint' },
    appearance: { title: 'onboarding.appearanceTitle', hint: 'onboarding.appearanceHint' },
    memory: { title: 'onboarding.memoryTitle', hint: 'onboarding.memoryHint' },
    java: { title: 'onboarding.javaTitle', hint: 'onboarding.javaHint' },
    model: { title: 'onboarding.modelTitle', hint: 'onboarding.modelHint' },
  };
  const current = headings[step];

  return (
    <div className={styles.page}>
      <div className={styles.shell}>
        {/*
          顶部进度轨。
          用等宽分段而不是数字「3 / 6」：用户关心的是「还剩几步」，一段一段的
          视觉长度比数字更快被读到，也顺带说明了总步数。
        */}
        <nav className={styles.progress} aria-label={t('onboarding.progress')}>
          {STEPS.map((id, position) => (
            <button
              key={id}
              type="button"
              className={`${styles.progressDot} ${position === index ? styles.progressCurrent : ''} ${position < index ? styles.progressDone : ''}`}
              onClick={() => goTo(id)}
              aria-current={position === index ? 'step' : undefined}
              aria-label={t('onboarding.stepOf', { values: { current: position + 1, total: STEPS.length } })}
            >
              <span />
            </button>
          ))}
        </nav>

        <header className={styles.head}>
          <span className={styles.stepIcon} aria-hidden="true">{stepIcon(step)}</span>
          <div>
            {/* tabIndex=-1 让它可被脚本聚焦，但不进入 Tab 序列。 */}
            <h1 ref={headingRef} tabIndex={-1} className={styles.title}>{t(current.title)}</h1>
            <p className={styles.hint}>{t(current.hint)}</p>
          </div>
        </header>

        <div className={styles.body}>
          {step === 'welcome' && (
            <div className={styles.welcome}>
              <div className={styles.logo}>MFL</div>
              <p className={styles.welcomeText}>{t('onboarding.welcomeBody')}</p>
              <ul className={styles.featureList}>
                {(['onboarding.featureInstall', 'onboarding.featureJava', 'onboarding.featureInstances', 'onboarding.featureAi'] as MessageKey[]).map(key => (
                  <li key={key}><Check size={14} aria-hidden="true" />{t(key)}</li>
                ))}
              </ul>
              <p className={styles.asideNote}>{t('onboarding.skipNote')}</p>
            </div>
          )}

          {step === 'language' && (
            <div className={styles.options} role="radiogroup" aria-label={t('language.title')}>
              {LANGUAGES.map(item => (
                <button
                  key={item.id}
                  type="button"
                  role="radio"
                  aria-checked={preference === item.id}
                  className={`${styles.option} ${preference === item.id ? styles.optionActive : ''}`}
                  onClick={() => setLanguage(item.id)}
                  lang={item.id === 'system' ? undefined : item.id}
                >
                  <span className={styles.optionLabel}>{item.native}</span>
                  {preference === item.id && <Check size={14} aria-hidden="true" />}
                </button>
              ))}
            </div>
          )}

          {step === 'appearance' && (
            <>
              <div className={styles.fieldGroup}>
                <span className={styles.fieldLabel}>{t('settings.theme')}</span>
                <div className={styles.segmented} role="radiogroup" aria-label={t('settings.theme')}>
                  {(['system', 'light', 'dark'] as const).map(id => (
                    <button
                      key={id}
                      type="button"
                      role="radio"
                      aria-checked={theme === id}
                      className={`${styles.segment} ${theme === id ? styles.segmentActive : ''}`}
                      /*
                       * 同时改两处：`onThemeChange` 让界面立刻换肤，`onPrefsChange`
                       * 把同一个值写进后端设置。只做前者的话，换肤有效但下次启动
                       * 会回到旧值；只做后者则要等一次保存往返才看得到效果。
                       */
                      onClick={() => {
                        onThemeChange(id);
                        onPrefsChange({ ...prefs, theme: id });
                      }}
                    >
                      {t(THEME_KEYS[id])}
                    </button>
                  ))}
                </div>
              </div>

              <div className={styles.fieldGroup}>
                <span className={styles.fieldLabel}>{t('settings.presets')}</span>
                <div className={styles.presets}>
                  {PRESETS.map(preset => {
                    const active = prefs.customAccent.toLowerCase() === preset.color;
                    return (
                      <button
                        key={preset.id}
                        type="button"
                        aria-pressed={active}
                        className={`${styles.preset} ${active ? styles.presetActive : ''}`}
                        onClick={() => {
                          onPrefsChange({ ...prefs, themePreset: preset.id, customAccent: preset.color });
                          onAccentChange(preset.color);
                        }}
                      >
                        <i style={{ background: preset.color }} aria-hidden="true" />
                        <span>{t(preset.labelKey)}</span>
                        {active && <Check size={13} aria-hidden="true" />}
                      </button>
                    );
                  })}
                </div>
              </div>

              {/*
                预览区。配色是这一屏唯一「只能用眼睛验收」的东西，所以给一块用
                当前强调色的样例，而不是只让用户看一排色块。
              */}
              <div className={styles.preview} style={{ ['--demo-accent' as string]: prefs.customAccent }} data-dark={dark ? 'true' : undefined}>
                <span className={styles.previewBadge}>{t('onboarding.previewAccent')}</span>
                <span className={styles.previewBar} aria-hidden="true" />
                <span className={styles.previewText}>{t('onboarding.previewSample')}</span>
              </div>
            </>
          )}

          {step === 'memory' && (
            <>
              <div className={styles.segmented} role="radiogroup" aria-label={t('memory.mode')}>
                {(['auto', 'custom'] as const).map(id => (
                  <button
                    key={id}
                    type="button"
                    role="radio"
                    aria-checked={prefs.memoryMode === id}
                    className={`${styles.segment} ${prefs.memoryMode === id ? styles.segmentActive : ''}`}
                    onClick={() => onPrefsChange({ ...prefs, memoryMode: id })}
                  >
                    {t(id === 'auto' ? 'memory.auto' : 'memory.custom')}
                  </button>
                ))}
              </div>

              <div className={`${styles.memoryControl} ${prefs.memoryMode === 'auto' ? styles.memoryDisabled : ''}`}>
                <div className={styles.memoryHead}>
                  <output className={styles.memoryValue}>{formatGb(effectiveMb / 1024)}</output>
                  <span className={styles.memoryTotal}>/ {formatGb(totalGb)}</span>
                </div>
                <input
                  className={styles.slider}
                  type="range"
                  min={MIN_SLIDER_INDEX}
                  max={maxIndex}
                  step={1}
                  value={sliderValue}
                  disabled={prefs.memoryMode === 'auto'}
                  aria-label={t('memory.max')}
                  aria-valuetext={formatGb(effectiveMb / 1024)}
                  style={{ ['--fill' as string]: `${((sliderValue - MIN_SLIDER_INDEX) / Math.max(1, maxIndex - MIN_SLIDER_INDEX)) * 100}%` }}
                  onChange={event => {
                    const mb = Math.max(MIN_MEMORY_MB, Math.round(sliderIndexToGb(Number(event.target.value)) * 1024));
                    onPrefsChange({ ...prefs, memoryMode: 'custom', memoryMb: mb });
                  }}
                />
              </div>

              <p className={styles.asideNote}>
                {prefs.memoryMode === 'auto'
                  ? t('onboarding.memoryAutoNote', { values: { size: formatGb(autoMb / 1024), total: formatGb(totalGb) } })
                  : t('onboarding.memoryCustomNote', { values: { total: formatGb(totalGb) } })}
              </p>
            </>
          )}

          {step === 'java' && (
            <>
              <div className={styles.javaStatus}>
                <Terminal size={16} aria-hidden="true" />
                <div>
                  <strong>{t('onboarding.javaFound', { count: runtimes.length })}</strong>
                  <span>
                    {javaBusy
                      ? t('onboarding.javaDetecting')
                      : javaNote
                        /*
                         * `note` 是工具层返回的「键 + 参数」，必须按当前语言渲染。
                         * 直接 `t(javaNote.note)` 只有在 note 恰好是字符串时才成立，
                         * 而它实际是对象——那样会静默显示成空。
                         */
                        ? renderLocalized(language, javaNote.note)
                        : t('onboarding.javaHint')}
                  </span>
                </div>
              </div>

              {runtimes.length > 0 && (
                <ul className={styles.javaList}>
                  {runtimes.slice(0, 4).map(runtime => (
                    <li key={runtime.path}>
                      <span className={styles.javaVersion}>{runtime.majorVersion}</span>
                      <span className={styles.javaPath} title={runtime.path}>{runtime.path}</span>
                    </li>
                  ))}
                </ul>
              )}

              <div className={styles.inlineActions}>
                <Button variant="ghost" loading={javaBusy} onClick={() => void runDetect()}>
                  {t('onboarding.javaRescan')}
                </Button>
                <span className={styles.asideNote}>{isDesktopShell() ? t('onboarding.javaManageLater') : t('onboarding.javaPreviewNote')}</span>
              </div>
            </>
          )}

          {step === 'model' && (
            <>
              <div className={styles.modelCard}>
                <Sparkles size={16} aria-hidden="true" />
                <div>
                  <strong>{model?.name ?? 'LFM3-1.7B'}</strong>
                  <span>
                    {model?.installed
                      ? t('onboarding.modelInstalled', { values: { size: formatBytes(model.expectedBytes) } })
                      : t('onboarding.modelMissing', { values: { size: formatBytes(model?.expectedBytes ?? 0) } })}
                  </span>
                </div>
                {model?.installed && <span className={styles.modelBadge}><Check size={12} aria-hidden="true" />{t('model.installed')}</span>}
              </div>
              <p className={styles.asideNote}>{t('onboarding.modelNote')}</p>

              {/*
                「跑了也跑不动」的提示。

                这一条比「未下载」优先：对一台内存不足的机器，引导用户去下 2 GB
                再用一个注定超时的功能，是这一屏最容易犯的错。所以当可行性判断说
                `notViable` 时，只说明原因，不给下载入口——而是说明「规则分析仍然
                可用」，那才是这台机器上真正能用的部分。
              */}
              {feasibility?.disableModel && (
                <div className={styles.modelWarn}>
                  <AlertTriangle size={15} aria-hidden="true" />
                  <div>
                    <strong>{t('onboarding.modelNotViable')}</strong>
                    <span>{feasibility.summary}</span>
                    <span>{feasibility.reason}</span>
                    <span>{t('onboarding.modelRulesStillWork')}</span>
                  </div>
                </div>
              )}

              {feasibility && !feasibility.disableModel && feasibility.feasibility === 'slow' && (
                <p className={styles.asideNote}>{t('onboarding.modelSlow', { values: { size: `${Math.round(feasibility.estimatedMb / 102.4) / 10} GB` } })}</p>
              )}

              {/*
                不在这里直接下 1.1 GB：那必须由用户在看到测速结果之后明确决定。
                但也不该只留下一句「去设置里找」——把入口做成一次点击，比让新用户
                在六个页签里找一个叫「下载」的地方友好得多。
              */}
              {!model?.installed && !feasibility?.disableModel && (
                <div className={styles.inlineActions}>
                  <Button variant="ghost" onClick={onOpenModelSettings}>
                    {t('onboarding.modelOpenSettings')}
                  </Button>
                  <span className={styles.asideNote}>{t('onboarding.modelLater')}</span>
                </div>
              )}
            </>
          )}
        </div>

        <footer className={styles.foot}>
          <div className={styles.footLeft}>
            {!isFirst && (
              <Button variant="ghost" icon={<ArrowLeft size={14} />} onClick={goBack}>
                {t('onboarding.back')}
              </Button>
            )}
          </div>
          <div className={styles.footRight}>
            {!isLast && (
              <Button variant="ghost" onClick={onDone}>{t('onboarding.skip')}</Button>
            )}
            {isLast ? (
              <Button icon={<ArrowRight size={15} />} onClick={onDone}>
                {t('app.getStarted')}
              </Button>
            ) : (
              <Button icon={<ArrowRight size={15} />} iconPosition="right" onClick={goNext}>
                {t('onboarding.next')}
              </Button>
            )}
          </div>
        </footer>
      </div>
    </div>
  );
}

/** 每一步左上角的图标。与侧栏、设置面板的图标语言保持一致。 */
function stepIcon(step: StepId) {
  switch (step) {
    case 'welcome': return <Sparkles size={18} />;
    case 'language': return <Languages size={18} />;
    case 'appearance': return <Palette size={18} />;
    case 'memory': return <MemoryStick size={18} />;
    case 'java': return <Terminal size={18} />;
    case 'model': return <Cpu size={18} />;
  }
}

function formatGb(gb: number): string {
  return `${gb.toFixed(1)} GB`;
}
