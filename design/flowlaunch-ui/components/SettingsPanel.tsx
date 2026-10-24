import React, { useEffect, useState } from 'react';
import { Activity, Check, Download, Gamepad2, HardDrive, Monitor, Palette, Plus, RotateCcw, Search, Settings2, ShieldCheck, SlidersHorizontal, Sparkles, Terminal, X } from 'lucide-react';
import type { GameInstance } from '../types';
import styles from './SettingsPanel.module.css';

type ThemeMode = 'light' | 'dark' | 'system';
type TabId = 'general' | 'game' | 'java' | 'download' | 'appearance' | 'advanced';
type ExperienceLevel = 'beginner' | 'standard' | 'expert';
interface LauncherPrefs {
  experienceLevel: ExperienceLevel; defaultVersionIsolation: boolean; gameResourceLocation: string; customSkin: string; launcherVisibility: 'keep' | 'hide' | 'close'; processPriority: 'normal' | 'high' | 'low'; windowPreset: 'default' | 'custom';
  fullscreen: boolean; javaAutoManage: boolean; gameArgs: string; preLaunchCommand: string; memoryMode: 'auto' | 'custom'; optimizeMemory: boolean; launchWrapperDisabled: boolean; lwjglUnsafeAgentDisabled: boolean; hardwareAcceleration: boolean;
  proxyMode: 'none' | 'system' | 'http'; proxyUrl: string; downloadTimeoutSec: number; downloadRetries: number; speedLimitKbps: number; verifySsl: boolean; downloadDirectory: string;
  debugMode: boolean; showCommandPreview: boolean; safeMode: boolean; logLevel: 'error' | 'warn' | 'info' | 'debug'; environmentVariables: string;
  communitySource: string; modpackFilenameFormat: string; modpackMergeMode: 'ask' | 'replace' | 'merge'; showGameHints: boolean; showDebugHints: boolean; autoLanguage: boolean;
  theme: ThemeMode; themePreset: string; customAccent: string; beginnerMode: boolean; showGameLog: boolean;
  javaPath: string; memoryMb: number; windowWidth: number; windowHeight: number; extraJvmArgs: string;
  downloadSource: string; maxConcurrentDownloads: number; connectionsPerFile: number; aria2Enabled: boolean;
  closeAfterLaunch: boolean; checkUpdates: boolean; gameDirectory: string; announcementPolicy: 'notify' | 'off'; cacheDirectory: string; anonymousUsage: boolean; opacity: number; hiddenTheme: boolean; showLauncherLogo: boolean; backgroundImage: string; backgroundMusic: boolean; titleBar: 'default' | 'text' | 'image' | 'none'; homeSource: string;
}
const defaults: LauncherPrefs = {
  experienceLevel: 'beginner', defaultVersionIsolation: true, gameResourceLocation: 'default', customSkin: 'pcl', launcherVisibility: 'keep', processPriority: 'normal', windowPreset: 'default',
  fullscreen: false, javaAutoManage: true, gameArgs: '', preLaunchCommand: '', memoryMode: 'custom', optimizeMemory: false, launchWrapperDisabled: false, lwjglUnsafeAgentDisabled: false, hardwareAcceleration: true,
  debugMode: false, showCommandPreview: false, safeMode: false, logLevel: 'info', environmentVariables: '',
  proxyMode: 'none', proxyUrl: '', downloadTimeoutSec: 60, downloadRetries: 3, speedLimitKbps: 0, verifySsl: true, downloadDirectory: '',
  communitySource: 'official', modpackFilenameFormat: '{name}-{version}', modpackMergeMode: 'ask', showGameHints: true, showDebugHints: false, autoLanguage: true,
  theme: 'dark', themePreset: 'moss', customAccent: '#41c98b', beginnerMode: true, showGameLog: true,
  javaPath: '', memoryMb: 4096, windowWidth: 1280, windowHeight: 720, extraJvmArgs: '',
  downloadSource: 'official', maxConcurrentDownloads: 4, connectionsPerFile: 4, aria2Enabled: true,
  closeAfterLaunch: false, checkUpdates: true, gameDirectory: '', announcementPolicy: 'notify', cacheDirectory: '', anonymousUsage: false, opacity: 100, hiddenTheme: false, showLauncherLogo: true, backgroundImage: '', backgroundMusic: false, titleBar: 'default', homeSource: 'official',
};
const tabs: { id: TabId; label: string; icon: React.ReactNode; caption: string }[] = [
  { id: 'general', label: '启动器', icon: <Settings2 />, caption: '启动器行为与基础选项' },
  { id: 'game', label: '游戏', icon: <Gamepad2 />, caption: '游戏窗口、日志与内存' },
  { id: 'java', label: 'Java', icon: <Terminal />, caption: '运行环境与虚拟机参数' },
  { id: 'download', label: '下载', icon: <Download />, caption: '下载源与并发控制' },
  { id: 'appearance', label: '外观', icon: <Palette />, caption: '主题、强调色与显示' },
  { id: 'advanced', label: '高级', icon: <SlidersHorizontal />, caption: '数据目录与高级行为' },
];

function readPrefs(): LauncherPrefs {
  try { return { ...defaults, ...JSON.parse(localStorage.getItem('flowlaunch-settings') || '{}') }; } catch { return defaults; }
}

export interface JavaRuntime { path: string; majorVersion: number; versionText: string }
type SettingsPanelProps = {
  theme: ThemeMode;
  onThemeChange: (theme: ThemeMode) => void;
  onClose: () => void;
  instance?: GameInstance;
  initialTab?: TabId;
  onJavaRuntimesChange?: (runtimes: JavaRuntime[]) => void;
  onGlobalJavaChange?: (path: string) => void;
  onInstanceJavaChange?: (instanceId: string, path: string) => void;
};

export function SettingsPanel({ theme, onThemeChange, onClose, instance, initialTab, onJavaRuntimesChange, onGlobalJavaChange, onInstanceJavaChange }: SettingsPanelProps) {
  const [prefs, setPrefs] = useState<LauncherPrefs>(() => ({ ...readPrefs(), theme }));
  const [javaRuntimes, setJavaRuntimes] = useState<JavaRuntime[]>(() => {
    try { return JSON.parse(localStorage.getItem('flowlaunch-java-runtimes') || '[]'); } catch { return []; }
  });
  const [instanceJavaPath, setInstanceJavaPath] = useState(() => {
    try { return JSON.parse(localStorage.getItem('flowlaunch-instance-java') || '{}')[instance?.id ?? ''] || ''; } catch { return ''; }
  });
  const [tab, setTab] = useState<TabId>(initialTab ?? (instance ? 'java' : 'general'));
  const [query, setQuery] = useState('');
  const [saved, setSaved] = useState(false);
  const [javaStatus, setJavaStatus] = useState('自动检测可用的 Java 运行环境');
  useEffect(() => {
    if (!instance) return;
    try { const saved = JSON.parse(localStorage.getItem('flowlaunch-instance-java') || '{}'); setInstanceJavaPath(saved[instance.id] || ''); } catch { setInstanceJavaPath(''); }
  }, [instance?.id]);
  useEffect(() => { setPrefs(current => ({ ...current, theme })); }, [theme]);
  useEffect(() => {
    document.documentElement.style.setProperty('--color-primary-500', prefs.customAccent);
    document.documentElement.style.setProperty('--color-primary-600', prefs.customAccent);
  }, [prefs.customAccent]);
  const update = <K extends keyof LauncherPrefs>(key: K, value: LauncherPrefs[K]) => {
    setPrefs(current => ({ ...current, [key]: value })); setSaved(false);
    if (key === 'theme') onThemeChange(value as ThemeMode);
    if (key === 'javaPath') onGlobalJavaChange?.(value as string);
  };
  const changeLevel = (level: ExperienceLevel) => {
    update('experienceLevel', level);
    update('beginnerMode', level === 'beginner');
  };
  const openSearchResult = () => {
    const text = query.toLowerCase();
    const routes: [string[], TabId][] = [
      [['java', 'jvm', '运行环境', '内存'], 'java'],
      [['下载', '镜像', '代理', '网络', '并发'], 'download'],
      [['窗口', '分辨率', '日志', '游戏'], 'game'],
      [['主题', '颜色', '外观'], 'appearance'],
      [['配置', '调试', '目录', '参数', '专家'], 'advanced'],
    ];
    setTab(routes.find(([terms]) => terms.some(term => text.includes(term)))?.[1] ?? 'general');
  };
  const save = () => {
    const nextPrefs = { ...prefs, beginnerMode: prefs.experienceLevel === 'beginner' };
    setPrefs(nextPrefs);
    localStorage.setItem('flowlaunch-settings', JSON.stringify(nextPrefs));
    onJavaRuntimesChange?.(javaRuntimes);
    onGlobalJavaChange?.(nextPrefs.javaPath);
    if (instance) {
      const instanceJava = JSON.parse(localStorage.getItem('flowlaunch-instance-java') || '{}');
      if (instanceJavaPath) instanceJava[instance.id] = instanceJavaPath; else delete instanceJava[instance.id];
      localStorage.setItem('flowlaunch-instance-java', JSON.stringify(instanceJava));
      onInstanceJavaChange?.(instance.id, instanceJavaPath);
    }
    // Keep the native command payload aligned with LauncherSettings in the Rust backend.
    localStorage.setItem('mfmcl-native-settings-draft', JSON.stringify({ schemaVersion: 3, themeMode: nextPrefs.theme, themePreset: nextPrefs.themePreset, customAccent: nextPrefs.customAccent, customBackground: nextPrefs.backgroundImage || null, beginnerMode: nextPrefs.beginnerMode, showGameLog: nextPrefs.showGameLog, javaPath: nextPrefs.javaPath || null, memoryMb: nextPrefs.memoryMode === 'auto' ? 2048 : nextPrefs.memoryMb, windowWidth: nextPrefs.windowWidth, windowHeight: nextPrefs.windowHeight, extraJvmArgs: nextPrefs.extraJvmArgs, gameArgs: nextPrefs.gameArgs, preLaunchCommand: nextPrefs.preLaunchCommand, memoryMode: nextPrefs.memoryMode, optimizeMemory: nextPrefs.optimizeMemory, processPriority: 'normal', fullscreen: nextPrefs.fullscreen, launcherVisibility: nextPrefs.launcherVisibility === 'hide' ? 'hide' : 'keep', defaultIsolated: nextPrefs.defaultVersionIsolation, closeAfterLaunch: nextPrefs.closeAfterLaunch, speedLimitKbps: nextPrefs.speedLimitKbps, downloadDirectory: nextPrefs.downloadDirectory || null, verifySsl: nextPrefs.verifySsl, communitySource: nextPrefs.communitySource, modpackFilenameFormat: nextPrefs.modpackFilenameFormat, modpackMergeMode: nextPrefs.modpackMergeMode, showGameHints: nextPrefs.showGameHints, showDebugHints: nextPrefs.showDebugHints, autoLanguage: nextPrefs.autoLanguage, announcementPolicy: nextPrefs.announcementPolicy, cacheDirectory: nextPrefs.cacheDirectory || null, anonymousUsage: nextPrefs.anonymousUsage, opacity: nextPrefs.opacity, hiddenTheme: nextPrefs.hiddenTheme, showLauncherLogo: nextPrefs.showLauncherLogo, backgroundImage: nextPrefs.backgroundImage || null, backgroundMusic: nextPrefs.backgroundMusic, titleBar: nextPrefs.titleBar, homeSource: nextPrefs.homeSource, garbageCollector: 'auto', updatePolicy: 'notify', download: { source: nextPrefs.downloadSource === 'auto' ? 'mixed' : nextPrefs.downloadSource, maxConcurrentDownloads: nextPrefs.maxConcurrentDownloads, connectionsPerFile: nextPrefs.connectionsPerFile, aria2Enabled: nextPrefs.aria2Enabled } }));
    setSaved(true); window.setTimeout(() => setSaved(false), 2400);
  };
  const reset = () => { setPrefs(defaults); onThemeChange(defaults.theme); setSaved(false); };
  const rememberJava = (runtime: JavaRuntime) => {
    const next = [runtime, ...javaRuntimes.filter(item => item.path !== runtime.path)];
    setJavaRuntimes(next);
    localStorage.setItem('flowlaunch-java-runtimes', JSON.stringify(next));
    onJavaRuntimesChange?.(next);
  };
  const detectJava = async () => {
    setJavaStatus('正在搜索 Java_HOME、系统路径及常见安装目录…');
    try {
      const invoke = (window as Window & { __TAURI__?: { core?: { invoke: <T>(command: string, args?: unknown) => Promise<T> } } }).__TAURI__?.core?.invoke;
      if (!invoke) { setJavaStatus('网页预览无法扫描本机；请在桌面版使用自动搜索或手动添加。'); return; }
      const runtimes = await invoke<JavaRuntime[]>('list_java_runtimes');
      const merged = [...runtimes, ...javaRuntimes.filter(item => !runtimes.some(runtime => runtime.path === item.path))];
      setJavaRuntimes(merged);
      localStorage.setItem('flowlaunch-java-runtimes', JSON.stringify(merged));
      onJavaRuntimesChange?.(merged);
      setJavaStatus(runtimes.length ? `找到 ${runtimes.length} 个 Java 运行环境；请在“全局 Java”中选择。` : '未找到 Java。可以手动添加已有的 Java 可执行文件。');
    } catch (error) { setJavaStatus(`搜索失败：${String(error)}`); }
  };
  const chooseManualJava = async () => {
    const picker = (window as Window & { __TAURI__?: { dialog?: { open: (options: unknown) => Promise<string | string[] | null> }; core?: { invoke: <T>(command: string, args?: unknown) => Promise<T> } } }).__TAURI__;
    if (!picker?.dialog || !picker.core) { setJavaStatus('网页预览无法读取本机文件；桌面版可手动添加 Java。'); return; }
    const selected = await picker.dialog.open({ multiple: false, title: '选择 Java 可执行文件' });
    if (typeof selected !== 'string') return;
    try {
      const runtime = await picker.core.invoke<{ path: string; majorVersion: number; versionText: string }>('inspect_java_path', { path: selected });
      rememberJava(runtime);
      update('javaPath', runtime.path);
      setJavaStatus(`已验证 Java ${runtime.majorVersion} · ${runtime.path}`);
    } catch (error) { setJavaStatus(`这个文件不是可用的 Java：${String(error)}`); }
  };
  const setInstanceRuntime = (path: string) => {
    setInstanceJavaPath(path);
    setSaved(false);
  };
  const browseJava = chooseManualJava;
  const visibleTabs = tabs.filter(item => `${item.label} ${item.caption}`.toLowerCase().includes(query.toLowerCase()));
  const toggle = (label: string, description: string, checked: boolean, onChange: (value: boolean) => void) => <div className={styles.row} key={label}><div className={styles.rowCopy}><strong>{label}</strong><span>{description}</span></div><button type="button" role="switch" aria-checked={checked} aria-label={label} className={`${styles.switch} ${checked ? styles.switchOn : ''}`} onClick={() => onChange(!checked)}><i /></button></div>;
  const field = (label: string, description: string, content: React.ReactNode) => <div className={styles.field} key={label}><div className={styles.rowCopy}><strong>{label}</strong><span>{description}</span></div><div className={styles.control}>{content}</div></div>;
  return <div className={styles.settings}>
    <aside className={styles.rail}>
      <div className={styles.railBrand}><div className={styles.brandMark}>FL</div><div><b>偏好设置</b><small>配置你的游戏环境</small></div></div>
      <label className={styles.search}><Search size={15}/><input value={query} onChange={e => setQuery(e.target.value)} onKeyDown={e=>e.key==='Enter'&&openSearchResult()} placeholder="搜索设置"/><kbd>Enter</kbd></label>
      <div className={styles.modePicker} aria-label="设置复杂度">{([['beginner','简易'],['standard','标准'],['expert','专家']] as [ExperienceLevel,string][]).map(([id,label])=><button key={id} onClick={()=>changeLevel(id)} className={prefs.experienceLevel===id?styles.modeSelected:''}>{label}</button>)}</div>
      <nav>{visibleTabs.length ? visibleTabs.map(item => <button key={item.id} className={`${styles.navItem} ${tab === item.id ? styles.navActive : ''}`} onClick={() => setTab(item.id)}>{item.icon}<span>{item.label}</span>{tab === item.id && <i/>}</button>) : <div className={styles.noResults}>没有匹配分类。按 Enter 搜索最相关设置。</div>}</nav>
      <div className={styles.railFoot}><ShieldCheck size={15}/><span>你的偏好仅保存在本机</span></div>
    </aside>
    <section className={styles.content}>
      <header className={styles.header}><div><div className={styles.eyebrow}>FLOWLAUNCH SETTINGS</div><h2>{tabs.find(item => item.id === tab)?.label}</h2><p>{tabs.find(item => item.id === tab)?.caption}</p></div><button className={styles.close} onClick={onClose} aria-label="关闭设置"><X size={19}/></button></header>
      <div className={styles.body}>
        {tab === 'general' && <><div className={styles.hero}><span className={styles.heroIcon}><Sparkles size={18}/></span><div><b>让启动器按你的习惯工作</b><p>调整启动行为和首次使用引导，设置会在保存后保留。</p></div></div><section className={styles.group}><h3>启动选项</h3>{field('默认版本隔离','每个实例使用独立的游戏、资源和存档目录，避免版本互相覆盖。',<select value={prefs.defaultVersionIsolation?'isolated':'shared'} onChange={e=>update('defaultVersionIsolation',e.target.value==='isolated')}><option value="isolated">隔离所有版本（推荐）</option><option value="shared">共享游戏目录</option></select>)}{field('游戏资源位置','资源文件默认保存在启动器数据目录，也可以在桌面版指定目录。',<select value={prefs.gameResourceLocation} onChange={e=>update('gameResourceLocation',e.target.value)}><option value="default">默认</option><option value="custom">自定义目录</option></select>)}{field('自定义皮肤','选择皮肤服务兼容模式。',<select value={prefs.customSkin} onChange={e=>update('customSkin',e.target.value)}><option value="pcl">PCL</option><option value="none">不使用</option><option value="custom">自定义 Yggdrasil</option></select>)}{field('启动器可见性','游戏启动后如何处理启动器窗口。',<select value={prefs.launcherVisibility} onChange={e=>update('launcherVisibility',e.target.value as LauncherPrefs['launcherVisibility'])}><option value="keep">游戏启动后仍保持不变</option><option value="hide">游戏启动后隐藏</option><option value="close">游戏启动后关闭</option></select>)}{field('进程优先级','高优先级可能改善游戏响应，但会增加系统负载。',<select value={prefs.processPriority} onChange={e=>update('processPriority',e.target.value as LauncherPrefs['processPriority'])}><option value="normal">普通</option><option value="high" disabled>高（暂未启用）</option><option value="low" disabled>低（暂未启用）</option></select>)}{field('窗口大小','启动时使用的默认窗口尺寸。',<select value={prefs.windowPreset} onChange={e=>update('windowPreset',e.target.value as LauncherPrefs['windowPreset'])}><option value="default">默认</option><option value="custom">自定义尺寸</option></select>)}</section><section className={styles.group}><h3>使用模式</h3><div className={styles.levelGrid}>{([['beginner','新手','只显示安全、推荐的选项'],['standard','普通玩家','平衡易用性与可调节性'],['expert','程序员','显示完整的启动、网络与调试控制']] as [ExperienceLevel,string,string][]).map(([id,label,description])=><button key={id} className={`${styles.levelCard} ${prefs.experienceLevel===id?styles.levelActive:''}`} onClick={()=>changeLevel(id)}><span className={styles.levelRadio}>{prefs.experienceLevel===id?'✓':''}</span><strong>{label}</strong><small>{description}</small></button>)}</div></section><section className={styles.group}><h3>启动行为</h3>{toggle('首次使用引导','显示创建实例、配置 Java 和开始游戏的入门步骤。',prefs.beginnerMode,v=>update('beginnerMode',v))}{toggle('检查启动器更新','启动时检查更新，有可用版本时提示你。',prefs.checkUpdates,v=>update('checkUpdates',v))}{toggle('启动游戏后关闭启动器','游戏启动成功后自动隐藏 FlowLaunch。',prefs.closeAfterLaunch,v=>update('closeAfterLaunch',v))}</section><section className={styles.group}><h3>辅助功能</h3>{toggle('游戏启动提示','显示 Java、内存和版本兼容性的友好提示。',prefs.showGameHints,v=>update('showGameHints',v))}{toggle('测试版启动提示','启动快照版或实验性配置时显示额外提醒。',prefs.showDebugHints,v=>update('showDebugHints',v))}{toggle('自动使用中文','根据系统语言自动选择中文界面。',prefs.autoLanguage,v=>update('autoLanguage',v))}</section><section className={styles.group}><h3>快捷信息</h3><div className={styles.infoCard}><Activity size={17}/><div><b>账户、游戏实例与下载</b><span>分别在侧栏的账户、游戏库和下载任务中管理。</span></div></div></section></>}
        {tab === 'game' && <><section className={styles.group}><h3>游戏窗口</h3>{field('默认窗口尺寸','Minecraft 首次启动时使用的窗口大小。',<div className={styles.dimensions}><input type="number" min="640" max="7680" value={prefs.windowWidth} onChange={e=>update('windowWidth',Number(e.target.value))}/><span>×</span><input type="number" min="360" max="4320" value={prefs.windowHeight} onChange={e=>update('windowHeight',Number(e.target.value))}/><small>像素</small></div>)}{toggle('显示游戏日志','实时查看游戏输出，并在启动器中保留日志文件。',prefs.showGameLog,v=>update('showGameLog',v))}{prefs.experienceLevel !== 'beginner' && toggle('默认全屏启动','启动 Minecraft 时自动进入全屏模式。',prefs.fullscreen,v=>update('fullscreen',v))}{prefs.experienceLevel === 'expert' && field('游戏参数覆盖','程序员模式：直接追加到 Minecraft 的游戏参数末尾。',<input value={prefs.gameArgs} onChange={e=>update('gameArgs',e.target.value)} placeholder="例如：--demo"/>)}</section><section className={styles.group}><h3>内存分配</h3>{field('内存模式','自动模式会根据系统内存选择安全值；自定义模式使用下方滑块。',<select value={prefs.memoryMode} onChange={e=>update('memoryMode',e.target.value as LauncherPrefs['memoryMode'])}><option value="auto">自动配置</option><option value="custom">自定义</option></select>)}{toggle('启动游戏前进行内存优化','启动前释放可回收的启动器缓存，为游戏预留更多内存。',prefs.optimizeMemory,v=>update('optimizeMemory',v))}{field('最大内存','为 Minecraft 分配的最大堆内存。建议为系统和启动器预留至少 2 GB。',<div className={styles.memory}><input type="range" min="512" max="16384" step="512" value={prefs.memoryMb} onChange={e=>update('memoryMb',Number(e.target.value))}/><output>{(prefs.memoryMb/1024).toFixed(prefs.memoryMb%1024?1:0)} GB</output></div>)}</section></>}
        {tab === 'java' && <><section className={styles.group}><h3>Java 运行环境</h3><div className={styles.javaCard}><div className={styles.javaIcon}><Terminal size={18}/></div><div><b>{prefs.javaPath ? `Java ${javaRuntimes.find(item => item.path === prefs.javaPath)?.majorVersion ?? ''} · 全局选择` : '按游戏版本自动选择'}</b><span>{javaStatus}</span></div><span className={styles.statusDot}/></div>{toggle('自动管理 Java','按 Minecraft 版本需求从已识别的运行环境中选择合适版本。关闭后使用全局指定 Java。',prefs.javaAutoManage,v=>update('javaAutoManage',v))}<div className={styles.javaActions}><button className={styles.javaAction} onClick={()=>void detectJava()}><Search size={15}/>自动搜索 Java</button><button className={styles.javaAction} onClick={()=>void chooseManualJava()}><Plus size={15}/>手动添加</button></div>{field('全局 Java','作为实例的默认运行环境；每个实例仍可单独覆盖。',<select value={prefs.javaPath} onChange={e=>{update('javaPath',e.target.value);setJavaStatus(e.target.value?'已选择全局 Java':'将按游戏版本自动选择')}}><option value="">自动选择（推荐）</option>{javaRuntimes.map(runtime=><option key={runtime.path} value={runtime.path}>Java {runtime.majorVersion} · {runtime.path}</option>)}</select>)}{javaRuntimes.length > 0 && <div className={styles.runtimeList} aria-label="已添加的 Java"><strong>已识别的 Java（{javaRuntimes.length}）</strong>{javaRuntimes.map(runtime=><div key={runtime.path}><span><b>Java {runtime.majorVersion}</b><small>{runtime.versionText} · {runtime.path}</small></span>{prefs.javaPath===runtime.path&&<em>全局使用中</em>}</div>)}</div>}{instance && <div className={styles.instanceJava}><div><strong>{instance.name} · 实例专属 Java</strong><span>为此实例单独指定运行环境；选择“跟随全局”会继承上方设置。</span></div><select aria-label={`${instance.name} 的 Java`} value={instanceJavaPath} onChange={e=>setInstanceRuntime(e.target.value)}><option value="">跟随全局设置（{prefs.javaPath ? `Java ${javaRuntimes.find(item=>item.path===prefs.javaPath)?.majorVersion ?? ''}` : '自动选择'}）</option>{javaRuntimes.map(runtime=><option key={runtime.path} value={runtime.path}>Java {runtime.majorVersion} · {runtime.path}</option>)}</select></div>}{toggle('禁用 Java Launch Wrapper','高级兼容选项：禁用启动包装器可能影响部分版本的启动诊断。',prefs.launchWrapperDisabled,v=>update('launchWrapperDisabled',v))}{toggle('禁用 LWJGL Unsafe Agent','仅在特定 Java 或安全策略冲突时使用。',prefs.lwjglUnsafeAgentDisabled,v=>update('lwjglUnsafeAgentDisabled',v))}{toggle('使用硬件加速','启用 WebView 和启动器界面的硬件加速。',prefs.hardwareAcceleration,v=>update('hardwareAcceleration',v))}{field('Java 可执行文件路径','可手动编辑路径，也可以通过上方按钮自动搜索或添加。',<div className={styles.pathControl}><input value={prefs.javaPath} onChange={e=>{update('javaPath',e.target.value);setJavaStatus('路径已修改；保存后作为全局 Java 生效')}} placeholder="留空：根据版本自动选择"/><button onClick={()=>void browseJava()}>浏览…</button>{prefs.javaPath&&<button className={styles.clearButton} onClick={()=>{update('javaPath','');setJavaStatus('将按游戏版本自动选择 Java')}}>清除</button>}</div>)}</section><section className={styles.group}><h3>虚拟机选项</h3>{field('额外 JVM 参数','每个参数以空格分隔；引号内的内容会作为单个参数。最大内存由上方选项控制。',<textarea rows={3} value={prefs.extraJvmArgs} onChange={e=>update('extraJvmArgs',e.target.value)} placeholder="例如：-XX:+UseG1GC -XX:最大GCPauseMillis=50"/>)}<p className={styles.note}>安全提示：请仅添加你了解的 JVM 参数。参数不会在启动器预览网页中执行。</p></section><section className={styles.group}><h3>高级启动参数</h3>{field('启动前执行命令','启动 Minecraft 前执行。桌面版会以当前实例目录作为工作目录。',<input value={prefs.preLaunchCommand} onChange={e=>update('preLaunchCommand',e.target.value)} placeholder="例如：./prepare-world.sh"/>)}<p className={styles.note}>请只填写可信命令；网页预览不会执行命令，桌面版会拒绝换行和 NUL 字符。</p></section></>}
        {tab === 'download' && <><section className={styles.group}><h3>内容来源</h3>{field('Minecraft 下载源','用于获取游戏版本清单和官方游戏文件。',<select value={prefs.downloadSource} onChange={e=>update('downloadSource',e.target.value)}><option value="official">官方源</option><option value="bmclapi">BMCLAPI 镜像</option><option value="auto">自动选择可用源</option></select>)}{field('代理模式','程序员模式：为下载请求设置系统代理或 HTTP 代理。',<select value={prefs.proxyMode} onChange={e=>update('proxyMode',e.target.value as LauncherPrefs['proxyMode'])}><option value="none">不使用代理</option><option value="system">跟随系统代理</option><option value="http">HTTP 代理</option></select>)}{prefs.proxyMode==='http' && field('代理地址','例如 http://127.0.0.1:7890。',<input value={prefs.proxyUrl} onChange={e=>update('proxyUrl',e.target.value)} placeholder="http://127.0.0.1:7890"/>)}{prefs.experienceLevel==='expert' && <>{field('连接超时','单个请求等待响应的时间。',<div className={styles.stepper}><button onClick={()=>update('downloadTimeoutSec',Math.max(10,prefs.downloadTimeoutSec-10))}>−</button><output>{prefs.downloadTimeoutSec}</output><button onClick={()=>update('downloadTimeoutSec',Math.min(600,prefs.downloadTimeoutSec+10))}>+</button><small>秒</small></div>)}{field('失败重试次数','网络错误时自动重试。',<div className={styles.stepper}><button onClick={()=>update('downloadRetries',Math.max(0,prefs.downloadRetries-1))}>−</button><output>{prefs.downloadRetries}</output><button onClick={()=>update('downloadRetries',Math.min(10,prefs.downloadRetries+1))}>+</button><small>次</small></div>)}</>}{field('最大下载速度','0 表示不限速，单位为 KB/s。',<input type="number" min="0" max="1048576" value={prefs.speedLimitKbps} onChange={e=>update('speedLimitKbps',Number(e.target.value))} placeholder="不限速"/>)}{toggle('验证 HTTPS 证书','建议保持开启，关闭后可能暴露下载内容被篡改的风险。',prefs.verifySsl,v=>update('verifySsl',v))}{field('目标文件夹','下载缓存和资源文件的保存目录。',<input value={prefs.downloadDirectory} onChange={e=>update('downloadDirectory',e.target.value)} placeholder="使用默认目录"/>)}{field('同时下载任务','同时进行的文件下载数量。',<div className={styles.stepper}><button onClick={()=>update('maxConcurrentDownloads',Math.max(1,prefs.maxConcurrentDownloads-1))}>−</button><output>{prefs.maxConcurrentDownloads}</output><button onClick={()=>update('maxConcurrentDownloads',Math.min(16,prefs.maxConcurrentDownloads+1))}>+</button><small>个任务</small></div>)}{field('单文件连接数','分段下载连接数，过高可能增加服务器负载。',<div className={styles.stepper}><button onClick={()=>update('connectionsPerFile',Math.max(1,prefs.connectionsPerFile-1))}>−</button><output>{prefs.connectionsPerFile}</output><button onClick={()=>update('connectionsPerFile',Math.min(16,prefs.connectionsPerFile+1))}>+</button><small>个连接</small></div>)}{toggle('使用 aria2 加速','可用时使用 aria2，失败时自动回退到校验型 HTTP 下载。',prefs.aria2Enabled,v=>update('aria2Enabled',v))}</section><section className={styles.group}><h3>社区资源</h3>{field('资源来源','Mod、资源包和整合包的搜索来源。',<select value={prefs.communitySource} onChange={e=>update('communitySource',e.target.value)}><option value="official">官方与内置来源</option><option value="modrinth">Modrinth</option><option value="curseforge">CurseForge</option></select>)}{field('整合包文件名格式','支持 {name} 和 {version} 占位符。',<input value={prefs.modpackFilenameFormat} onChange={e=>update('modpackFilenameFormat',e.target.value)} />)}{field('整合包合并策略','导入已有实例时如何处理重复文件。',<select value={prefs.modpackMergeMode} onChange={e=>update('modpackMergeMode',e.target.value as LauncherPrefs['modpackMergeMode'])}><option value="ask">每次询问</option><option value="replace">覆盖</option><option value="merge">合并</option></select>)}</section><div className={styles.infoCard}><HardDrive size={17}/><div><b>文件完整性校验</b><span>下载文件按官方 SHA-1 校验；镜像源不会跳过校验。</span></div></div></>}
        {tab === 'appearance' && <><section className={styles.group}><h3>显示与背景</h3>{field('界面透明度','调整启动器窗口透明度。',<div className={styles.memory}><input type="range" min="60" max="100" step="1" value={prefs.opacity} onChange={e=>update('opacity',Number(e.target.value))}/><output>{prefs.opacity}%</output></div>)}{toggle('显示启动器图标','在标题栏和欢迎页显示 FlowLaunch 图标。',prefs.showLauncherLogo,v=>update('showLauncherLogo',v))}{toggle('启用背景音乐','播放启动器背景音乐；默认关闭。',prefs.backgroundMusic,v=>update('backgroundMusic',v))}{field('标题栏样式','选择启动器顶部标题栏的显示方式。',<select value={prefs.titleBar} onChange={e=>update('titleBar',e.target.value as LauncherPrefs['titleBar'])}><option value="default">默认</option><option value="text">文本</option><option value="image">图片</option><option value="none">隐藏</option></select>)}{field('主页内容来源','主页资讯和内容卡片的来源。',<select value={prefs.homeSource} onChange={e=>update('homeSource',e.target.value)}><option value="official">官方内容</option><option value="none">不显示</option><option value="custom">自定义来源</option></select>)}</section><section className={styles.group}><h3>颜色模式</h3>{field('界面主题','选择你偏好的显示模式。',<select value={prefs.theme} onChange={e=>update('theme',e.target.value as ThemeMode)}><option value="system">跟随系统</option><option value="dark">深色</option><option value="light">浅色</option></select>)}</section><section className={styles.group}><h3>强调色</h3>{field('主题色','为选中项、主要按钮和进度状态选择强调色。',<div className={styles.colorControl}><div className={styles.swatches}>{['#41c98b','#8b7cff','#45a7e8','#f19b52','#e76b8a'].map(color=><button key={color} aria-label={color} className={prefs.customAccent===color?styles.swatchSelected:''} style={{'--swatch':color} as React.CSSProperties} onClick={()=>update('customAccent',color)}/>)}</div><input type="color" value={prefs.customAccent} onChange={e=>update('customAccent',e.target.value)}/><code>{prefs.customAccent.toUpperCase()}</code></div>)}</section><section className={styles.group}><h3>主题预设</h3><div className={styles.presets}>{[['moss','苔原绿','#41c98b'],['ocean','深海蓝','#45a7e8'],['lavender','暮色紫','#8b7cff'],['ember','余烬橙','#f19b52']].map(([id,name,color])=><button key={id} className={`${styles.preset} ${prefs.themePreset===id?styles.presetActive:''}`} onClick={()=>{update('themePreset',id);update('customAccent',color)}}><i style={{background:color}}/><span>{name}</span>{prefs.themePreset===id&&<b>✓</b>}</button>)}</div></section></>}
        {tab === 'advanced' && <><section className={styles.group}><h3>数据与诊断</h3>{field('游戏数据目录','实例、游戏资源和下载缓存的根目录。',<div className={styles.pathControl}><input value={prefs.gameDirectory} onChange={e=>update('gameDirectory',e.target.value)} placeholder="使用启动器默认目录"/><button onClick={()=>setJavaStatus('网页预览不能读取本机目录；桌面版本将使用系统文件选择器。')}>浏览…</button></div>)}{field('缓存文件夹','启动器下载缓存和临时文件的位置。',<input value={prefs.cacheDirectory} onChange={e=>update('cacheDirectory',e.target.value)} placeholder="使用默认目录"/>)}{field('启动器更新','是否在启动时检查新版本。',<select value={prefs.announcementPolicy} onChange={e=>update('announcementPolicy',e.target.value as LauncherPrefs['announcementPolicy'])}><option value="notify">检查并提示</option><option value="off">关闭更新提示</option></select>)}{toggle('匿名数据收集','仅收集崩溃和性能统计，不包含账户、存档或访问令牌。',prefs.anonymousUsage,v=>update('anonymousUsage',v))}{toggle('安全模式','启动时暂时禁用自定义 JVM 参数与第三方加载器，用于排查启动问题。',prefs.safeMode,v=>update('safeMode',v))}{toggle('显示启动命令预览','启动前显示最终 Java 命令，便于复制到终端排查。',prefs.showCommandPreview,v=>update('showCommandPreview',v))}</section><section className={styles.group}><h3>程序员工具</h3>{toggle('调试模式','记录更详细的启动器与下载诊断信息。',prefs.debugMode,v=>update('debugMode',v))}{field('日志级别','控制启动器日志的详细程度。',<select value={prefs.logLevel} onChange={e=>update('logLevel',e.target.value as LauncherPrefs['logLevel'])}><option value="error">仅错误</option><option value="warn">警告及以上</option><option value="info">常规信息</option><option value="debug">完整调试</option></select>)}{field('环境变量','每行一个 KEY=VALUE，仅在启动 Minecraft 时生效。',<textarea rows={4} value={prefs.environmentVariables} onChange={e=>update('environmentVariables',e.target.value)} placeholder="JAVA_TOOL_OPTIONS=-Dfile.encoding=UTF-8\nMALLOC_ARENA_MAX=2"/>)}<p className={styles.note}>环境变量与 JVM 参数会在启动前校验；网页预览不会执行它们。桌面版会避免把令牌写入日志。</p></section><section className={styles.group}><h3>维护</h3><button className={styles.reset} onClick={reset}><RotateCcw size={15}/>恢复此页设置默认值</button></section></>}
      </div>
      <footer className={styles.footer}><span className={saved?styles.saved:''}>{saved?'✓ 已保存到此设备':'更改尚未保存'}</span><div><button className={styles.cancel} onClick={onClose}>关闭</button><button className={styles.save} onClick={save}>{saved?'已保存':'保存设置'}</button></div></footer>
    </section>
  </div>;
}
