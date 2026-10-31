import React, { useEffect, useRef, useState } from 'react';
import { Activity, Download, Gamepad2, HardDrive, Palette, RotateCcw, Search, Settings2, ShieldCheck, SlidersHorizontal, Terminal, X } from 'lucide-react';
import type { GameInstance } from '../types';
import { applyAccent as applyAccentRamp } from '../utils/accent';
import { JavaSettingsSection } from './JavaSettingsSection';
import { MemorySection } from './MemorySection';
import { DEFAULT_PREFS, SETTINGS_STORAGE_KEY, readPrefs, type LauncherPrefs, type ThemeMode } from '../utils/launcherPrefs';
import { LanguageSection } from './LanguageSection';
import { SkinSection } from './SkinSection';
import { useI18n } from '../utils/i18n';
import { aria2Status, errorText, type Aria2Availability } from '../utils/native';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import { loc, type Localized } from '../utils/i18n/localized';
import {
  DEFAULT_INSTANCE_JAVA,
  loadInstanceJavaSettings,
  loadJavaList,
  saveInstanceJavaSettings,
  saveJavaList,
  sortDiscovered,
  type InstanceJavaSetting,
  type JavaRuntime,
} from '../utils/java';
import { detectJavaRuntimes, importJavaRuntime, revealJavaFolder } from '../utils/javaBridge';
import { LauncherUpdateCard } from './LauncherUpdateCard';
import { BackgroundSection } from './BackgroundSection';
import { CurseForgeKeyCard } from './CurseForgeKeyCard';
import { readBackgroundSetting, saveBackgroundSetting, type BackgroundSetting } from '../utils/background';
import styles from './SettingsPanel.module.css';

type TabId = 'general' | 'game' | 'java' | 'download' | 'appearance' | 'advanced';
export type { JavaRuntime } from '../utils/java';
// 标签与说明从词典取，这里只保留结构和图标。
const tabs: { id: TabId; labelKey: MessageKey; captionKey: MessageKey; icon: React.ReactNode }[] = [
  { id: 'general', labelKey: 'settings.tab.general', captionKey: 'settings.tab.generalCaption', icon: <Settings2 /> },
  { id: 'game', labelKey: 'settings.tab.game', captionKey: 'settings.tab.gameCaption', icon: <Gamepad2 /> },
  { id: 'java', labelKey: 'settings.tab.java', captionKey: 'settings.tab.javaCaption', icon: <Terminal /> },
  { id: 'download', labelKey: 'settings.tab.download', captionKey: 'settings.tab.downloadCaption', icon: <Download /> },
  { id: 'appearance', labelKey: 'settings.tab.appearance', captionKey: 'settings.tab.appearanceCaption', icon: <Palette /> },
  { id: 'advanced', labelKey: 'settings.tab.advanced', captionKey: 'settings.tab.advancedCaption', icon: <SlidersHorizontal /> },
];

function applyAccent(color: string) {
  applyAccentRamp(color);
}
type SettingsPanelProps = {
  theme: ThemeMode;
  onThemeChange: (theme: ThemeMode) => void;
  onClose: () => void;
  instance?: GameInstance;
  initialTab?: TabId;
  onJavaRuntimesChange?: (runtimes: JavaRuntime[]) => void;
  onGlobalJavaChange?: (path: string) => void;
  onInstanceJavaChange?: (instanceId: string, setting: InstanceJavaSetting) => void;
  /** 把界面偏好交给上层写入后端 `LauncherSettings`；失败时抛错以阻止「已保存」状态。 */
  onSaveSettings?: (prefs: LauncherPrefs) => Promise<void>;
  /**
   * 背景设置变化。
   *
   * 与 `onSaveSettings` 分开：背景的二进制在 IndexedDB 里，元数据在 localStorage
   * 里，两者都不经过后端。而且它需要**即时生效**——用户拖一下遮罩滑条就该看到
   * 效果，不能等到点保存。
   */
  onBackgroundChange?: (setting: BackgroundSetting) => void;
  onNotice?: (message: string) => void;
  /** 当前解析出的主题，供背景预览选底色。 */
  dark: boolean;
};

export function SettingsPanel({ theme, onThemeChange, onClose, instance, initialTab, onJavaRuntimesChange, onGlobalJavaChange, onInstanceJavaChange, onSaveSettings, onBackgroundChange, onNotice, dark }: SettingsPanelProps) {
  const { t, language } = useI18n();
  const [prefs, setPrefs] = useState<LauncherPrefs>(() => {
    // 从未保存过强调色时，回落到屏幕上实际生效的值，而不是硬编码的默认色。
    const currentAccent = getComputedStyle(document.documentElement).getPropertyValue('--color-primary-500').trim();
    return { ...readPrefs(currentAccent), theme };
  });
  const [javaRuntimes, setJavaRuntimes] = useState<JavaRuntime[]>(loadJavaList);
  const [instanceJavaSetting, setInstanceJavaSetting] = useState<InstanceJavaSetting>(() => {
    const all = loadInstanceJavaSettings();
    return instance ? all[instance.id] ?? { ...DEFAULT_INSTANCE_JAVA } : { ...DEFAULT_INSTANCE_JAVA };
  });
  const [javaBusy, setJavaBusy] = useState(false);
  const [tab, setTab] = useState<TabId>(initialTab ?? (instance ? 'java' : 'general'));
  /**
   * 背景设置。
   *
   * 独立于 `prefs`：`prefs` 是一个整体提交给后端的表单，而背景需要即时生效。
   * 混在一起会导致「拖遮罩滑条 → 背景变了 → 点关闭 → 背景没变回去」这种
   * 半保存状态，而这正是设置面板已经为强调色避免过的坑。
   */
  const [background, setBackground] = useState<BackgroundSetting>(readBackgroundSetting);

  /**
   * 背景变化：立即落盘并通知外层。
   *
   * 不等待「保存」按钮：背景是一眼可见的东西，用户拖滑条的时候就应该看到结果。
   * 这一步也不可能失败到需要回滚——最坏情况是隐私模式下本次会话有效。
   */
  const changeBackground = (next: BackgroundSetting) => {
    setBackground(next);
    saveBackgroundSetting(next);
    onBackgroundChange?.(next);
  };

  /**
   * 切换页签时把内容区滚回顶部。
   *
   * 为什么必须显式做：React 复用同一个 DOM 节点时，滚动位置会被保留。用户在
   * 「游戏」页签滚到中间，切到「下载」后会停在另一页的同一个像素位置——而两组
   * 内容的长度与顺序完全不同，于是落点往往是某个不相干条目的中段（实测停在
   * 「内容来源」，页首的「游戏窗口」反而看不到）。
   *
   * 这不会丢失用户的位置感：页签是并列的目的地，不是同一条长列表的分段，
   * 用户期望的是「打开这一页并从头看」。
   *
   * 依赖 `tab` 而不是在 onClick 里调用：页签也可能由搜索路由（openSearchResult）
   * 改变，放在 effect 里两条路径都能覆盖。
   */
  useEffect(() => {
    bodyRef.current?.scrollTo({ top: 0 });
  }, [tab]);
  const [query, setQuery] = useState('');
  const [saved, setSaved] = useState(false);
  useEffect(() => {
    let alive = true;
    void aria2Status()
      .then(result => { if (alive) setAria2(result); })
      .catch(() => { if (alive) setAria2(null); });
    return () => { alive = false; };
  }, []);
  /** 正在等待后端确认。用于禁用保存按钮，避免连点产生并发写入。 */
  const [saving, setSaving] = useState(false);
  /** aria2 实际可用性。开关打开但二进制缺失/校验失败时，必须让用户看见。 */
  const [aria2, setAria2] = useState<Aria2Availability | null>(null);
  /** 后端拒绝保存的原因（例如内存超上限），显示在页脚上方。 */
  const [saveError, setSaveError] = useState('');
  const [javaStatus, setJavaStatus] = useState<Localized>(loc('java.detectIdle'));
  const [dragOffset, setDragOffset] = useState({ x: 0, y: 0 });
  const panelRef = useRef<HTMLDivElement>(null);
  /** 可滚动的内容区。切换页签时要把它的滚动位置归零。 */
  const bodyRef = useRef<HTMLDivElement>(null);
  const dragState = useRef<{ pointerId: number; startX: number; startY: number; originX: number; originY: number } | null>(null);
  // Snapshot the values that are actually persisted. Closing without saving
  // restores them instead of leaving live-preview changes behind.
  const savedThemeRef = useRef<ThemeMode>(theme);
  const savedAccentRef = useRef(prefs.customAccent);
  const accentReadyRef = useRef(false);
  useEffect(() => {
    if (!instance) { setInstanceJavaSetting({ ...DEFAULT_INSTANCE_JAVA }); return; }
    const all = loadInstanceJavaSettings();
    setInstanceJavaSetting(all[instance.id] ?? { ...DEFAULT_INSTANCE_JAVA });
  }, [instance?.id]);
  useEffect(() => { setPrefs(current => ({ ...current, theme })); }, [theme]);
  useEffect(() => {
    // Skip the initial mount: the App already applied the persisted accent
    // before paint, and applying it again here made Settings look like it
    // changed the theme as soon as it opened.
    if (!accentReadyRef.current) { accentReadyRef.current = true; return; }
    applyAccent(prefs.customAccent);
  }, [prefs.customAccent]);
  const update = <K extends keyof LauncherPrefs>(key: K, value: LauncherPrefs[K]) => {
    setPrefs(current => ({ ...current, [key]: value })); setSaved(false);
    if (key === 'theme') onThemeChange(value as ThemeMode);
    if (key === 'javaPath') onGlobalJavaChange?.(value as string);
  };
  const openSearchResult = () => {
    const text = query.toLowerCase();
    // 关键词按当前语言取，切到英文/日文后也能用当地词搜到对应分类。
    const routes: [MessageKey, TabId][] = [
      ['settings.routeJava', 'java'],
      ['settings.routeDownload', 'download'],
      ['settings.routeGame', 'game'],
      ['settings.routeAppearance', 'appearance'],
      ['settings.routeAdvanced', 'advanced'],
    ];
    setTab(routes.find(([key]) => t(key).split(/\s+/).some(term => term && text.includes(term)))?.[1] ?? 'general');
  };
  /**
   * 保存设置。
   *
   * 关键的一步是 `onSaveSettings`：它把界面偏好翻译成后端 `LauncherSettings`
   * 并真正落库。之前这里只写 localStorage，还往 localStorage 塞了一份没人读的
   * 「原生设置草稿」——界面显示「已保存」，而启动器实际使用的仍是旧值，
   * 用户只有在下次启动发现设置没生效时才会察觉。
   *
   * 顺序上先等后端成功再标记「已保存」：后端会做范围校验，失败时必须让用户
   * 看到失败，而不是看到一个绿色的成功状态。
   */
  const save = async () => {
    const nextPrefs = { ...prefs };
    setPrefs(nextPrefs);
    localStorage.setItem(SETTINGS_STORAGE_KEY, JSON.stringify(nextPrefs));
    onJavaRuntimesChange?.(javaRuntimes);
    onGlobalJavaChange?.(nextPrefs.javaPath);
    saveJavaList(javaRuntimes);
    if (instance) {
      const all = loadInstanceJavaSettings();
      all[instance.id] = instanceJavaSetting;
      saveInstanceJavaSettings(all);
      onInstanceJavaChange?.(instance.id, instanceJavaSetting);
    }

    setSaving(true);
    try {
      await onSaveSettings?.(nextPrefs);
      savedThemeRef.current = nextPrefs.theme;
      savedAccentRef.current = nextPrefs.customAccent;
      setSaveError('');
      setSaved(true);
      window.setTimeout(() => setSaved(false), 2400);
    } catch (error) {
      // 保留面板打开，并把后端拒绝的原因显示出来——例如内存超过本机上限。
      setSaveError(errorText(error, '保存设置失败。'));
    } finally {
      setSaving(false);
    }
  };
  const close = () => {
    /*
     * 关闭时撤销「未保存」的主题与强调色预览。
     *
     * 但「跟随背景配色」是个例外：那个强调色不是面板里的临时预览，而是背景带的
     * 属性，关掉面板它也该继续生效。只在手动配色时才还原到打开面板时的值。
     */
    const autoAccentActive = background.accentMode === 'auto';
    if (!autoAccentActive) applyAccent(savedAccentRef.current);
    if (theme !== savedThemeRef.current) onThemeChange(savedThemeRef.current);
    /*
     * 背景本身不回滚：它已经落盘了。
     *
     * 这是它与主题/强调色的关键区别——后两者是为了「先看看效果」才做实时预览，
     * 关闭即撤销；背景的选择（选哪张图、遮罩多厚）是一次明确的操作，用户不会
     * 期望关掉面板就丢失。
     */
    onClose();
  };
  const clampOffset = (value: number, max: number) => Math.max(-max, Math.min(max, value));
  const startDrag = (event: React.PointerEvent<HTMLElement>) => {
    if (event.button !== 0 || (event.target as HTMLElement).closest('button, a, input, select, textarea, [role="button"]')) return;
    dragState.current = { pointerId: event.pointerId, startX: event.clientX, startY: event.clientY, originX: dragOffset.x, originY: dragOffset.y };
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const moveDrag = (event: React.PointerEvent<HTMLElement>) => {
    const drag = dragState.current;
    const panel = panelRef.current;
    if (!drag || drag.pointerId !== event.pointerId || !panel) return;
    const maxX = Math.max(0, (window.innerWidth - panel.offsetWidth) / 2 - 8);
    const maxY = Math.max(0, (window.innerHeight - panel.offsetHeight) / 2 - 8);
    setDragOffset({
      x: clampOffset(drag.originX + event.clientX - drag.startX, maxX),
      y: clampOffset(drag.originY + event.clientY - drag.startY, maxY),
    });
  };
  const endDrag = (event: React.PointerEvent<HTMLElement>) => {
    if (dragState.current?.pointerId !== event.pointerId) return;
    dragState.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
  };
  useEffect(() => {
    const resetPosition = () => setDragOffset({ x: 0, y: 0 });
    window.addEventListener('resize', resetPosition);
    return () => window.removeEventListener('resize', resetPosition);
  }, []);
  /*
   * 恢复默认设置。
   *
   * 只重置表单字段，不动背景：背景有自己的「清空」按钮，而且它的素材是几十 MB
   * 的二进制，混进这个按钮里会让「恢复默认」变成一次不可预期的磁盘清理。
   */
  const reset = () => { setPrefs(DEFAULT_PREFS); onThemeChange(DEFAULT_PREFS.theme); setSaved(false); };
  const persistRuntimes = (next: JavaRuntime[]) => {
    setJavaRuntimes(next);
    saveJavaList(next);
    onJavaRuntimesChange?.(next);
  };
  const detectJava = async () => {
    setJavaBusy(true);
    setJavaStatus(loc('java.detectSearching'));
    const result = await detectJavaRuntimes();
    // Keep the user's ordering first; append anything new at the bottom, which is
    // what PCL2 does after a refresh (new entries must be re-prioritised by hand).
    const fresh = sortDiscovered(result.runtimes.filter(runtime => !javaRuntimes.some(item => item.path === runtime.path)));
    const merged = [...javaRuntimes, ...fresh];
    persistRuntimes(merged);
    setJavaStatus(result.note);
    setJavaBusy(false);
  };
  const importJava = async () => {
    setJavaBusy(true);
    const result = await importJavaRuntime(language);
    if (result.runtime) {
      // PCL2 inserts an imported Java at the top of the list.
      persistRuntimes([result.runtime, ...javaRuntimes.filter(item => item.path !== result.runtime!.path)]);
    }
    if (result.note) setJavaStatus(result.note);
    setJavaBusy(false);
  };
  const revealJava = async (runtime: JavaRuntime) => {
    setJavaStatus(await revealJavaFolder(runtime));
  };
  // 搜索只匹配当前语言的标签与说明；关键词路由由 openSearchResult 负责。
  const visibleTabs = tabs.filter(item => `${t(item.labelKey)} ${t(item.captionKey)}`.toLowerCase().includes(query.toLowerCase()));
  const toggle = (label: string, description: string, checked: boolean, onChange: (value: boolean) => void) => {
    return <div className={styles.row} key={label}><div className={styles.rowCopy}><strong>{label}</strong><span>{description}</span></div><button type="button" role="switch" aria-checked={checked} aria-label={label} className={`${styles.switch} ${checked ? styles.switchOn : ''}`} onClick={() => onChange(!checked)}><i /></button></div>;
  };
  const field = (label: string, description: string, content: React.ReactNode) => {
    return <div className={styles.field} key={label}><div className={styles.rowCopy}><strong>{label}</strong><span>{description}</span></div><div className={styles.control}>{content}</div></div>;
  };

  const currentTab = tabs.find(item => item.id === tab);

  const generalPage = (
    <>
      <section className={styles.group}>
        <h3>{t('settings.launcherOptions')}</h3>
        {field(t('settings.versionIsolation'), t('settings.versionIsolationHint'),
          <select value={prefs.defaultVersionIsolation ? 'isolated' : 'shared'} onChange={e => update('defaultVersionIsolation', e.target.value === 'isolated')}>
            <option value="isolated">{t('settings.isolated')}</option>
            <option value="shared">{t('settings.shared')}</option>
          </select>)}
        {field(t('settings.resourceLocation'), t('settings.resourceLocationHint'),
          <select value={prefs.gameResourceLocation} onChange={e => update('gameResourceLocation', e.target.value)}>
            <option value="default">{t('settings.default')}</option>
            <option value="custom">{t('settings.customDir')}</option>
          </select>)}
        {field(t('settings.launcherVisibility'), t('settings.launcherVisibilityHint'),
          <select value={prefs.launcherVisibility} onChange={e => update('launcherVisibility', e.target.value as LauncherPrefs['launcherVisibility'])}>
            <option value="keep">{t('settings.visibilityKeep')}</option>
            <option value="hide">{t('settings.visibilityHide')}</option>
            <option value="close">{t('settings.visibilityClose')}</option>
          </select>)}
        {field(t('settings.processPriority'), t('settings.processPriorityHint'),
          <select value={prefs.processPriority} onChange={e => update('processPriority', e.target.value as LauncherPrefs['processPriority'])}>
            <option value="normal">{t('settings.priorityNormal')}</option>
            <option value="high" disabled>{t('settings.priorityHigh')}</option>
            <option value="low" disabled>{t('settings.priorityLow')}</option>
          </select>)}
        {field(t('settings.windowSize'), t('settings.windowSizeHint'),
          <select value={prefs.windowPreset} onChange={e => update('windowPreset', e.target.value as LauncherPrefs['windowPreset'])}>
            <option value="default">{t('settings.windowDefault')}</option>
            <option value="custom">{t('settings.windowCustom')}</option>
          </select>)}
      </section>
      <section className={styles.group}>
        <h3>{t('settings.startupBehavior')}</h3>
        {toggle(t('settings.checkUpdates'), t('settings.checkUpdatesHint'), prefs.checkUpdates, v => update('checkUpdates', v))}
        {toggle(t('settings.closeAfterLaunch'), t('settings.closeAfterLaunchHint'), prefs.closeAfterLaunch, v => update('closeAfterLaunch', v))}
      </section>
      <section className={styles.group}>
        <h3>{t('settings.accessibility')}</h3>
        {toggle(t('settings.gameHints'), t('settings.gameHintsHint'), prefs.showGameHints, v => update('showGameHints', v))}
        {toggle(t('settings.debugHints'), t('settings.debugHintsHint'), prefs.showDebugHints, v => update('showDebugHints', v))}
      </section>
      <section className={styles.group}>
        <h3>{t('settings.quickInfo')}</h3>
        <div className={styles.infoCard}><Activity size={17} /><div><b>{t('settings.quickInfoTitle')}</b><span>{t('settings.quickInfoHint')}</span></div></div>
      </section>
    </>
  );

  const gamePage = (
    <>
      <section className={styles.group}>
        <h3>{t('settings.gameWindow')}</h3>
        {field(t('settings.windowDimensions'), t('settings.windowDimensionsHint'),
          <div className={styles.dimensions}>
            <input type="number" min="640" max="7680" value={prefs.windowWidth} onChange={e => update('windowWidth', Number(e.target.value))} />
            <span>×</span>
            <input type="number" min="360" max="4320" value={prefs.windowHeight} onChange={e => update('windowHeight', Number(e.target.value))} />
            <small>{t('settings.pixels')}</small>
          </div>)}
        {toggle(t('settings.showLog'), t('settings.showLogHint'), prefs.showGameLog, v => update('showGameLog', v))}
        {toggle(t('settings.fullscreen'), t('settings.fullscreenHint'), prefs.fullscreen, v => update('fullscreen', v))}
        {field(t('settings.gameArgs'), t('settings.gameArgsHint'),
          <input value={prefs.gameArgs} onChange={e => update('gameArgs', e.target.value)} placeholder={t('settings.gameArgsPlaceholder')} />)}
      </section>
      <SkinSection setting={prefs.skin} onChange={v => update('skin', v)} />
      <MemorySection
        mode={prefs.memoryMode}
        memoryMb={prefs.memoryMb}
        optimizeMemory={prefs.optimizeMemory}
        modCount={instance?.modCount}
        onModeChange={v => update('memoryMode', v)}
        onMemoryChange={v => update('memoryMb', v)}
        onOptimizeChange={v => update('optimizeMemory', v)}
      />
    </>
  );

  const javaPage = (
    <>
      <JavaSettingsSection
        runtimes={javaRuntimes}
        globalPath={prefs.javaPath}
        status={javaStatus}
        busy={javaBusy}
        instance={instance}
        instanceSetting={instanceJavaSetting}
        onRuntimesChange={next => { persistRuntimes(next); setSaved(false); }}
        onGlobalPathChange={path => { update('javaPath', path); setSaved(false); }}
        onInstanceSettingChange={setting => { setInstanceJavaSetting(setting); setSaved(false); }}
        onDetect={() => void detectJava()}
        onImport={() => void importJava()}
        onReveal={runtime => void revealJava(runtime)}
      />
      <section className={styles.group}>
        <h3>{t('settings.jvmOptions')}</h3>
        {field(t('settings.extraJvm'), t('settings.extraJvmHint'),
          <textarea rows={3} value={prefs.extraJvmArgs} onChange={e => update('extraJvmArgs', e.target.value)} placeholder={t('settings.jvmPlaceholder')} />)}
        <p className={styles.note}>{t('settings.jvmSafety')}</p>
      </section>
      <section className={styles.group}>
        <h3>{t('settings.advancedLaunch')}</h3>
        {field(t('settings.preLaunch'), t('settings.preLaunchHint'),
          <input value={prefs.preLaunchCommand} onChange={e => update('preLaunchCommand', e.target.value)} placeholder={t('settings.preLaunchPlaceholder')} />)}
        <p className={styles.note}>{t('settings.preLaunchSafety')}</p>
      </section>
    </>
  );

  const downloadPage = (
    <>
      <section className={styles.group}>
        <h3>{t('settings.contentSource')}</h3>
        {field(t('settings.downloadSource'), t('settings.downloadSourceHint'),
          <select value={prefs.downloadSource} onChange={e => update('downloadSource', e.target.value)}>
            {/*
              「自动」排在第一位并作为默认值：它对应后端的镜像优先 + 官方回退，
              是唯一会根据网络状况自动选择快的那条路的方式。另外两项是用户明确
              要求只走某一边时才需要的，排在后面。
            */}
            <option value="auto">{t('settings.sourceAuto')}</option>
            <option value="official">{t('settings.sourceOfficial')}</option>
            <option value="bmclapi">{t('settings.sourceBmclapi')}</option>
          </select>)}
        {field(t('settings.proxyMode'), t('settings.proxyModeHint'),
          <select value={prefs.proxyMode} onChange={e => update('proxyMode', e.target.value as LauncherPrefs['proxyMode'])}>
            <option value="none">{t('settings.proxyNone')}</option>
            <option value="system">{t('settings.proxySystem')}</option>
            <option value="http">{t('settings.proxyHttp')}</option>
          </select>)}
        {prefs.proxyMode === 'http' && field(t('settings.proxyUrl'), t('settings.proxyUrlHint'),
          <input value={prefs.proxyUrl} onChange={e => update('proxyUrl', e.target.value)} placeholder="http://127.0.0.1:7890" />)}
        {field(t('settings.timeout'), t('settings.timeoutHint'),
          <div className={styles.stepper}>
            <button onClick={() => update('downloadTimeoutSec', Math.max(10, prefs.downloadTimeoutSec - 10))}>−</button>
            <output>{prefs.downloadTimeoutSec}</output>
            <button onClick={() => update('downloadTimeoutSec', Math.min(600, prefs.downloadTimeoutSec + 10))}>+</button>
            <small>{t('settings.seconds')}</small>
          </div>)}
        {field(t('settings.retries'), t('settings.retriesHint'),
          <div className={styles.stepper}>
            <button onClick={() => update('downloadRetries', Math.max(0, prefs.downloadRetries - 1))}>−</button>
            <output>{prefs.downloadRetries}</output>
            <button onClick={() => update('downloadRetries', Math.min(10, prefs.downloadRetries + 1))}>+</button>
            <small>{t('settings.times')}</small>
          </div>)}
        {field(t('settings.speedLimit'), t('settings.speedLimitHint'),
          <input type="number" min="0" max="1048576" value={prefs.speedLimitKbps} onChange={e => update('speedLimitKbps', Number(e.target.value))} placeholder={t('settings.unlimited')} />)}
        {toggle(t('settings.verifySsl'), t('settings.verifySslHint'), prefs.verifySsl, v => update('verifySsl', v))}
        {field(t('settings.targetFolder'), t('settings.targetFolderHint'),
          <input value={prefs.downloadDirectory} onChange={e => update('downloadDirectory', e.target.value)} placeholder={t('settings.useDefaultDir')} />)}
        {field(t('settings.concurrentTasks'), t('settings.concurrentTasksHint'),
          <div className={styles.stepper}>
            <button onClick={() => update('maxConcurrentDownloads', Math.max(1, prefs.maxConcurrentDownloads - 1))}>−</button>
            <output>{prefs.maxConcurrentDownloads}</output>
            <button onClick={() => update('maxConcurrentDownloads', Math.min(16, prefs.maxConcurrentDownloads + 1))}>+</button>
            <small>{t('settings.tasksUnit')}</small>
          </div>)}
        {field(t('settings.connections'), t('settings.connectionsHint'),
          <div className={styles.stepper}>
            <button onClick={() => update('connectionsPerFile', Math.max(1, prefs.connectionsPerFile - 1))}>−</button>
            <output>{prefs.connectionsPerFile}</output>
            <button onClick={() => update('connectionsPerFile', Math.min(16, prefs.connectionsPerFile + 1))}>+</button>
            <small>{t('settings.connectionsUnit')}</small>
          </div>)}
        {toggle(t('settings.aria2'), t('settings.aria2Hint'), prefs.aria2Enabled, v => update('aria2Enabled', v))}
        {/*
          aria2 的真实状态。
          只在开关打开时显示「不可用」的原因：用户没启用加速时，附带与否与他无关，
          把一个红条常驻在那里只会造成困扰。
        */}
        {prefs.aria2Enabled && aria2 && !aria2.bundled && (
          <div className={styles.inlineNote} role="status">
            <strong>{t('settings.aria2Unavailable')}</strong>
            <span>{aria2.problem ?? t('settings.aria2Fallback')}</span>
          </div>
        )}
        {prefs.aria2Enabled && aria2?.bundled && (
          <div className={styles.inlineNoteOk} role="status">
            <span>{t('settings.aria2Ready', { values: { version: aria2.version ?? '1.37.0' } })}</span>
          </div>
        )}
      </section>
      {/*
        CurseForge 的 Key。

        放在「下载」页：「从哪拿到内容」与上面的下载源、代理、并发是同一类设置。

        这一块补的是一个真实的缺口。之前配置入口只在资源包/地图分类里出现（那两类
        只走 CurseForge，没 key 就整页是表单）；模组分类只在**没有** key 时显示
        入口。内置 key 随构建发行之后，「已可用」成了默认状态，于是模组页永远不显示
        配置入口——内置 key 配额耗尽或被轮换时，用户在那里找不到任何能操作的东西，
        只能等新版本。设置里有一处常驻入口才是正确的兜底。
      */}
      <CurseForgeKeyCard onNotice={onNotice} />
      <section className={styles.group}>
        <h3>{t('settings.community')}</h3>
        {field(t('settings.communitySource'), t('settings.communitySourceHint'),
          <select value={prefs.communitySource} onChange={e => update('communitySource', e.target.value)}>
            <option value="official">{t('settings.communityBuiltin')}</option>
            <option value="modrinth">Modrinth</option>
            <option value="curseforge">CurseForge</option>
          </select>)}
        {field(t('settings.modpackFormat'), t('settings.modpackFormatHint'),
          <input value={prefs.modpackFilenameFormat} onChange={e => update('modpackFilenameFormat', e.target.value)} />)}
        {field(t('settings.modpackMerge'), t('settings.modpackMergeHint'),
          <select value={prefs.modpackMergeMode} onChange={e => update('modpackMergeMode', e.target.value as LauncherPrefs['modpackMergeMode'])}>
            <option value="ask">{t('settings.mergeAsk')}</option>
            <option value="replace">{t('settings.mergeReplace')}</option>
            <option value="merge">{t('settings.mergeMerge')}</option>
          </select>)}
      </section>
      <div className={styles.infoCard}><HardDrive size={17} /><div><b>{t('settings.integrity')}</b><span>{t('settings.integrityHint')}</span></div></div>
    </>
  );

  const appearancePage = (
    <>
      <LanguageSection />
      {/*
        背景排在最前：它是「外观」页里最重的一项，用户在别的页签翻来翻去之后
        回到这里，最先要找的多半是它。
      */}
      <BackgroundSection
        setting={background}
        onChange={changeBackground}
        onNotice={onNotice ?? (() => undefined)}
        dark={dark}
      />
      <section className={styles.group}>
        <h3>{t('settings.display')}</h3>
        {field(t('settings.opacity'), t('settings.opacityHint'),
          <div className={styles.memory}>
            <input type="range" min="60" max="100" step="1" value={prefs.opacity} onChange={e => update('opacity', Number(e.target.value))} />
            <output>{prefs.opacity}%</output>
          </div>)}
        {toggle(t('settings.showLogo'), t('settings.showLogoHint'), prefs.showLauncherLogo, v => update('showLauncherLogo', v))}
        {toggle(t('settings.backgroundMusic'), t('settings.backgroundMusicHint'), prefs.backgroundMusic, v => update('backgroundMusic', v))}
        {field(t('settings.titleBar'), t('settings.titleBarHint'),
          <select value={prefs.titleBar} onChange={e => update('titleBar', e.target.value as LauncherPrefs['titleBar'])}>
            <option value="default">{t('settings.titleBarDefault')}</option>
            <option value="text">{t('settings.titleBarText')}</option>
            <option value="image">{t('settings.titleBarImage')}</option>
            <option value="none">{t('settings.titleBarNone')}</option>
          </select>)}
        {field(t('settings.homeSource'), t('settings.homeSourceHint'),
          <select value={prefs.homeSource} onChange={e => update('homeSource', e.target.value)}>
            <option value="official">{t('settings.homeOfficial')}</option>
            <option value="none">{t('settings.homeNone')}</option>
            <option value="custom">{t('settings.homeCustom')}</option>
          </select>)}
      </section>
      <section className={styles.group}>
        <h3>{t('settings.colorMode')}</h3>
        {field(t('settings.theme'), t('settings.themeHint'),
          <select value={prefs.theme} onChange={e => update('theme', e.target.value as ThemeMode)}>
            <option value="system">{t('settings.themeSystem')}</option>
            <option value="dark">{t('settings.themeDark')}</option>
            <option value="light">{t('settings.themeLight')}</option>
          </select>)}
      </section>
      <section className={styles.group}>
        <h3>{t('settings.accent')}</h3>
        {field(t('settings.accentColor'), t('settings.accentColorHint'),
          <div className={styles.colorControl}>
            <div className={styles.swatches}>
              {['#41c98b', '#8b7cff', '#53b4f6', '#f19b52', '#e76b8a'].map(color => (
                <button key={color} aria-label={color} className={prefs.customAccent === color ? styles.swatchSelected : ''} style={{ '--swatch': color } as React.CSSProperties} onClick={() => update('customAccent', color)} />
              ))}
            </div>
            <input type="color" value={prefs.customAccent} onChange={e => update('customAccent', e.target.value)} />
            <code>{prefs.customAccent.toUpperCase()}</code>
          </div>)}
      </section>
      <section className={styles.group}>
        <h3>{t('settings.presets')}</h3>
        <div className={styles.presets}>
          {([['moss', t('settings.presetMoss'), '#41c98b'], ['ocean', t('settings.presetOcean'), '#53b4f6'], ['lavender', t('settings.presetLavender'), '#8b7cff'], ['ember', t('settings.presetEmber'), '#f19b52']] as const).map(([id, name, color]) => (
            <button key={id} className={`${styles.preset} ${prefs.themePreset === id ? styles.presetActive : ''}`} onClick={() => { update('themePreset', id); update('customAccent', color); }}>
              <i style={{ background: color }} /><span>{name}</span>{prefs.themePreset === id && <b>✓</b>}
            </button>
          ))}
        </div>
      </section>
    </>
  );

  const advancedPage = (
    <>
      <section className={styles.group}>
        <h3>{t('settings.dataDiagnostics')}</h3>
        {field(t('settings.gameDir'), t('settings.gameDirHint'),
          <div className={styles.pathControl}>
            <input value={prefs.gameDirectory} onChange={e => update('gameDirectory', e.target.value)} placeholder={t('settings.useLauncherDefault')} />
            <button onClick={() => setJavaStatus(t('settings.browseUnavailable'))}>{t('settings.browse')}</button>
          </div>)}
        {field(t('settings.cacheDir'), t('settings.cacheDirHint'),
          <input value={prefs.cacheDirectory} onChange={e => update('cacheDirectory', e.target.value)} placeholder={t('settings.useDefaultDir')} />)}
        <LauncherUpdateCard />
        {field(t('settings.updatePolicy'), t('settings.updatePolicyHint'),
          <select value={prefs.announcementPolicy} onChange={e => update('announcementPolicy', e.target.value as LauncherPrefs['announcementPolicy'])}>
            <option value="notify">{t('settings.updateNotify')}</option>
            <option value="off">{t('settings.updateOff')}</option>
          </select>)}
        {toggle(t('settings.anonymous'), t('settings.anonymousHint'), prefs.anonymousUsage, v => update('anonymousUsage', v))}
        {toggle(t('settings.safeMode'), t('settings.safeModeHint'), prefs.safeMode, v => update('safeMode', v))}
        {toggle(t('settings.commandPreview'), t('settings.commandPreviewHint'), prefs.showCommandPreview, v => update('showCommandPreview', v))}
      </section>
      <section className={styles.group}>
        <h3>{t('settings.debugTools')}</h3>
        {toggle(t('settings.debugMode'), t('settings.debugModeHint'), prefs.debugMode, v => update('debugMode', v))}
        {field(t('settings.logLevel'), t('settings.logLevelHint'),
          <select value={prefs.logLevel} onChange={e => update('logLevel', e.target.value as LauncherPrefs['logLevel'])}>
            <option value="error">{t('settings.logError')}</option>
            <option value="warn">{t('settings.logWarn')}</option>
            <option value="info">{t('settings.logInfo')}</option>
            <option value="debug">{t('settings.logDebug')}</option>
          </select>)}
        {field(t('settings.envVars'), t('settings.envVarsHint'),
          <textarea rows={4} value={prefs.environmentVariables} onChange={e => update('environmentVariables', e.target.value)} placeholder={t('settings.envVarsPlaceholder')} />)}
        <p className={styles.note}>{t('settings.envVarsNote')}</p>
      </section>
      <section className={styles.group}>
        <h3>{t('settings.maintenance')}</h3>
        <button className={styles.reset} onClick={reset}><RotateCcw size={15} />{t('settings.resetSection')}</button>
      </section>
    </>
  );

  const pages: Record<TabId, React.ReactNode> = {
    general: generalPage,
    game: gamePage,
    java: javaPage,
    download: downloadPage,
    appearance: appearancePage,
    advanced: advancedPage,
  };

  return <div ref={panelRef} className={styles.settings} role="dialog" aria-modal="true" aria-label={t('settings.title')} style={{ transform: `translate(calc(-50% + ${dragOffset.x}px), calc(-50% + ${dragOffset.y}px))` }}>
    <aside className={styles.rail}>
      <div className={styles.railBrand}><div className={styles.brandMark}>MFL</div><div><b>{t('settings.title')}</b><small>{t('settings.subtitle')}</small></div></div>
      <label className={styles.search}>
        <Search size={15} />
        <input value={query} onChange={e => setQuery(e.target.value)} onKeyDown={e => e.key === 'Enter' && openSearchResult()} placeholder={t('settings.searchPlaceholder')} />
        <kbd>Enter</kbd>
      </label>
      <nav>
        {visibleTabs.length
          ? visibleTabs.map(item => (
            <button key={item.id} className={`${styles.navItem} ${tab === item.id ? styles.navActive : ''}`} onClick={() => setTab(item.id)}>
              {item.icon}<span>{t(item.labelKey)}</span>{tab === item.id && <i />}
            </button>
          ))
          : <div className={styles.noResults}>{t('settings.noResults')}</div>}
      </nav>
      <div className={styles.railFoot}><ShieldCheck size={15} /><span>{t('settings.localOnly')}</span></div>
    </aside>
    <section className={styles.content}>
      <header className={styles.header} onPointerDown={startDrag} onPointerMove={moveDrag} onPointerUp={endDrag} onPointerCancel={endDrag} onDoubleClick={() => setDragOffset({ x: 0, y: 0 })}>
        <div>
          <div className={styles.eyebrow}>{t('settings.eyebrow')}</div>
          <h2>{currentTab ? t(currentTab.labelKey) : ''}</h2>
          <p>{currentTab ? t(currentTab.captionKey) : ''}</p>
        </div>
        <button className={styles.close} onClick={close} aria-label={t('settings.closeAria')}><X size={19} /></button>
      </header>
      <div className={styles.body} tabIndex={0} aria-label={t('settings.bodyAria')} ref={bodyRef}>
        {/*
          保存失败的横幅放在正文末尾而不是弹一条提示：设置面板是多页签的长表单，
          用户在最后一页点保存，如果错误只出现在一闪而过的 toast 里，很容易错过，
          然后以为已经保存成功。这里把后端拒绝的原因留在页面上，直到下次保存。
        */}
        {saveError && (
          <div className={styles.saveError} role="alert">
            <strong>{t('settings.saveFailed')}</strong>
            <span>{saveError}</span>
          </div>
        )}
        {pages[tab]}
      </div>
      <footer className={styles.footer}>
        <span className={saved ? styles.saved : ''}>
          {saving ? t('settings.saving') : saved ? t('settings.saved') : t('settings.unsaved')}
        </span>
        <div>
          <button className={styles.cancel} onClick={close}>{t('settings.closeButton')}</button>
          <button className={styles.save} onClick={() => void save()} disabled={saving}>
            {saved ? t('settings.savedButton') : t('settings.saveButton')}
          </button>
        </div>
      </footer>
    </section>
  </div>;
}
