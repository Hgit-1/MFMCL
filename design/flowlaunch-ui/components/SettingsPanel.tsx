import React, { useEffect, useRef, useState } from 'react';
import { Activity, Download, Gamepad2, HardDrive, Palette, RotateCcw, Search, Settings2, ShieldCheck, SlidersHorizontal, Terminal, X } from 'lucide-react';
import type { GameInstance } from '../types';
import { applyAccent as applyAccentRamp } from '../utils/accent';
import { JavaSettingsSection } from './JavaSettingsSection';
import { autoAllocateMb, deviceMemoryMbSync, lastKnownUsedMb } from '../utils/memory';
import { MemorySection } from './MemorySection';
import { DEFAULT_PREFS, SETTINGS_STORAGE_KEY, readPrefs, type LauncherPrefs, type ThemeMode } from '../utils/launcherPrefs';
import { LanguageSection } from './LanguageSection';
import { SkinSection } from './SkinSection';
import { useI18n } from '../utils/i18n';
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
};

export function SettingsPanel({ theme, onThemeChange, onClose, instance, initialTab, onJavaRuntimesChange, onGlobalJavaChange, onInstanceJavaChange }: SettingsPanelProps) {
  const { t, language, preference } = useI18n();
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
  const [query, setQuery] = useState('');
  const [saved, setSaved] = useState(false);
  const [javaStatus, setJavaStatus] = useState<Localized>(loc('java.detectIdle'));
  const [dragOffset, setDragOffset] = useState({ x: 0, y: 0 });
  const panelRef = useRef<HTMLDivElement>(null);
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
  const save = () => {
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
    // Keep the native command payload aligned with LauncherSettings in the Rust backend.
    localStorage.setItem('mfmcl-native-settings-draft', JSON.stringify({ schemaVersion: 3, themeMode: nextPrefs.theme, themePreset: nextPrefs.themePreset, customAccent: nextPrefs.customAccent, customBackground: nextPrefs.backgroundImage || null, beginnerMode: false, showGameLog: nextPrefs.showGameLog, javaPath: nextPrefs.javaPath || null, memoryMb: nextPrefs.memoryMode === 'auto' ? autoAllocateMb({ totalMb: deviceMemoryMbSync(), modCount: instance?.modCount, usedMb: lastKnownUsedMb() }) : nextPrefs.memoryMb, windowWidth: nextPrefs.windowWidth, windowHeight: nextPrefs.windowHeight, extraJvmArgs: nextPrefs.extraJvmArgs, gameArgs: nextPrefs.gameArgs, preLaunchCommand: nextPrefs.preLaunchCommand, memoryMode: nextPrefs.memoryMode, optimizeMemory: nextPrefs.optimizeMemory, processPriority: 'normal', fullscreen: nextPrefs.fullscreen, launcherVisibility: nextPrefs.launcherVisibility === 'hide' ? 'hide' : 'keep', defaultIsolated: nextPrefs.defaultVersionIsolation, closeAfterLaunch: nextPrefs.closeAfterLaunch, speedLimitKbps: nextPrefs.speedLimitKbps, downloadDirectory: nextPrefs.downloadDirectory || null, verifySsl: nextPrefs.verifySsl, communitySource: nextPrefs.communitySource, modpackFilenameFormat: nextPrefs.modpackFilenameFormat, modpackMergeMode: nextPrefs.modpackMergeMode, showGameHints: nextPrefs.showGameHints, showDebugHints: nextPrefs.showDebugHints, autoLanguage: preference === 'system', announcementPolicy: nextPrefs.announcementPolicy, cacheDirectory: nextPrefs.cacheDirectory || null, anonymousUsage: nextPrefs.anonymousUsage, opacity: nextPrefs.opacity, hiddenTheme: nextPrefs.hiddenTheme, showLauncherLogo: nextPrefs.showLauncherLogo, backgroundImage: nextPrefs.backgroundImage || null, backgroundMusic: nextPrefs.backgroundMusic, titleBar: nextPrefs.titleBar, homeSource: nextPrefs.homeSource, garbageCollector: 'auto', updatePolicy: 'notify', download: { source: nextPrefs.downloadSource === 'auto' ? 'mixed' : nextPrefs.downloadSource, maxConcurrentDownloads: nextPrefs.maxConcurrentDownloads, connectionsPerFile: nextPrefs.connectionsPerFile, aria2Enabled: nextPrefs.aria2Enabled } }));
    savedThemeRef.current = nextPrefs.theme;
    savedAccentRef.current = nextPrefs.customAccent;
    setSaved(true); window.setTimeout(() => setSaved(false), 2400);
  };
  const close = () => {
    // "关闭" discards unsaved live previews (theme + accent).
    applyAccent(savedAccentRef.current);
    if (theme !== savedThemeRef.current) onThemeChange(savedThemeRef.current);
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
            <option value="official">{t('settings.sourceOfficial')}</option>
            <option value="bmclapi">{t('settings.sourceBmclapi')}</option>
            <option value="auto">{t('settings.sourceAuto')}</option>
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
      </section>
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
      <div className={styles.body} tabIndex={0} aria-label={t('settings.bodyAria')}>
        {pages[tab]}
      </div>
      <footer className={styles.footer}>
        <span className={saved ? styles.saved : ''}>{saved ? t('settings.saved') : t('settings.unsaved')}</span>
        <div>
          <button className={styles.cancel} onClick={close}>{t('settings.closeButton')}</button>
          <button className={styles.save} onClick={save}>{saved ? t('settings.savedButton') : t('settings.saveButton')}</button>
        </div>
      </footer>
    </section>
  </div>;
}
