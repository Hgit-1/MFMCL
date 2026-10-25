import React, { useState, useEffect, useRef } from 'react';
import { Check, ChevronDown } from 'lucide-react';
import { useTheme } from '../hooks/useTheme';
import { Button } from '../components/Button';
import { Card } from '../components/Card';
import { AccountModal } from '../components/AccountModal';
import { DefaultUserAvatar } from '../components/DefaultUserAvatar';
import { SettingsPanel } from '../components/SettingsPanel';
import { InstanceSettings } from '../components/InstanceSettings';
import { StoreWorkspace, STORE_CATEGORIES } from '../components/StoreWorkspace';
import { PluginWorkspace } from '../components/PluginWorkspace';
import { LOCALES, useI18n } from '../utils/i18n';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import { readPrefs, type LauncherPrefs } from '../utils/launcherPrefs';
import { allPlugins, loadPluginStore, pluginNeedsAttention, savePluginStore, type PluginStore } from '../utils/plugins';
import {
  EMPTY_OVERRIDES,
  loadInstanceOverrides,
  saveInstanceOverrides,
  type InstanceOverrides,
  type OverrideMap,
} from '../utils/instanceOverrides';
import { BlockIcon } from '../components/BlockIcon';
import { VANILLA_BLOCK } from '../utils/store/blocks';
import { Image as ImageIcon, Map, Package, Puzzle } from 'lucide-react';
import {
  DEFAULT_INSTANCE_JAVA,
  loadInstanceJavaSettings,
  loadJavaList,
  saveInstanceJavaSettings,
  type InstanceJavaSetting,
  type JavaRuntime,
} from '../utils/java';
import type { GameInstance, Account, AppContext, DownloadTask } from '../types';
import type { StoreCategory } from '../utils/store/types';
import { CATEGORY_KEYS } from '../utils/store/types';
import styles from './App.module.css';
import '../styles/globals.css';

// 图标组件（简化版）
const PlayIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor"><path d="M6 4l10 6-10 6V4z"/></svg>;
const PlusIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><path d="M10 5v10M5 10h10"/></svg>;
const SettingsIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><circle cx="10" cy="10" r="3"/><path d="M10 1v2m0 14v2M1 10h2m14 0h2m-3.5-6.5l1.4-1.4m-11.8 0l-1.4-1.4m12.6 12.6l1.4 1.4m-11.8 0l-1.4 1.4"/></svg>;
const LibraryIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><rect x="2" y="3" width="16" height="14" rx="2"/><path d="M2 7h16"/></svg>;
const ShopIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><path d="M3 6l3-3h8l3 3v11a2 2 0 01-2 2H5a2 2 0 01-2-2V6z"/><path d="M3 6h14v2a3 3 0 01-6 0 3 3 0 01-6 0V6"/></svg>;
const DownloadIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><path d="M10 3v10m-4-4l4 4 4-4M3 17h14"/></svg>;
/*
 * 插件图标：一枚插座。
 *
 * 插件在中文语境里也常被读成「扩展/接口」，插座比拼图块更直白地传达「插上去
 * 就多一个能力」，而且与商店用的盒子、库用的矩形在轮廓上区分得开——侧栏图标
 * 只有 20px，靠轮廓差异认类比靠细节更重要。
 */
const PluginsIcon = () => (
  <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M7 2v4M13 2v4" />
    <path d="M5 6h10v3a5 5 0 01-5 5 5 5 0 01-5-5V6z" />
    <path d="M10 14v4" />
  </svg>
);
function SidebarAccountAvatar({ account }: { account?: Account }) {
  const { t } = useI18n();
  if (!account?.avatar) return <DefaultUserAvatar className={styles.defaultUserAvatar} />;
  return (
    <img
      className={styles.sidebarAvatar}
      src={account.avatar}
      alt={t('account.avatarAlt', { values: { name: account.username } })}
    />
  );
}

const SunIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><circle cx="10" cy="10" r="4"/><path d="M10 2v2m0 12v2M2 10h2m12 0h2m-2.8-5.2l1.4-1.4m-11.2 0L4 4.8m11.2 11.2l1.4 1.4m-11.2 0L4 15.2"/></svg>;
const MoonIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor"><path d="M17.293 13.293A8 8 0 016.707 2.707a8.001 8.001 0 1010.586 10.586z"/></svg>;

/**
 * 退出登录图标。
 *
 * 用业界通行的「门 + 右向箭头」造型（Material Symbols 的 logout、GitHub 与
 * VS Code 用的都是这一种），而不是「人 + 减号」：后者容易被读成「删除账户」。
 * 描边取 1.9、圆头圆角，与侧栏其它 lucide 图标（20px / strokeWidth 2）的
 * 视觉重量对齐。
 */
const SignOutIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
    <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
    <path d="M16 17l5-5-5-5" />
    <path d="M21 12H9" />
  </svg>
);

const TASK_STATUS_KEYS: Record<DownloadTask['status'], MessageKey> = {
  queued: 'app.taskQueued',
  downloading: 'app.taskDownloading',
  verifying: 'app.taskVerifying',
  installing: 'app.taskInstalling',
  completed: 'app.taskCompleted',
  failed: 'app.taskFailed',
  paused: 'app.taskPaused',
  cancelled: 'app.taskCancelled',
};

/** 仍在进行中的任务状态。已完成/失败/取消的任务不再计入侧栏徽章。 */
const ACTIVE_TASK_STATUSES: DownloadTask['status'][] = ['queued', 'downloading', 'verifying', 'installing'];

const CATEGORY_ICONS: Record<Exclude<StoreCategory, 'vanilla'>, React.ReactNode> = {
  mod: <Puzzle size={18} strokeWidth={1.8} />,
  modpack: <Package size={18} strokeWidth={1.8} />,
  resourcepack: <ImageIcon size={18} strokeWidth={1.8} />,
  world: <Map size={18} strokeWidth={1.8} />,
};

// 只用来排序/遍历分类，标签由 t(CATEGORY_KEYS[id]) 提供，不在这里存文案。
const storeCategories = STORE_CATEGORIES;


function App() {
  const { theme, setTheme, resolvedTheme } = useTheme();
  const { t, language } = useI18n();
  const locale = LOCALES[language];
  const [context, setContext] = useState<AppContext>('first-run');
  const [instances, setInstances] = useState<GameInstance[]>([]);
  const [selectedInstance, setSelectedInstance] = useState<GameInstance | undefined>();
  const [currentAccount, setCurrentAccount] = useState<Account | undefined>();
  const [recentAccounts, setRecentAccounts] = useState<Account[]>([
    {
      id: 'recent-1',
      username: 'Steve2023',
      uuid: 'recent-uuid-1',
      avatar: '/demo-avatars/notch-head.png',
      type: 'microsoft',
      hasMinecraft: true,
    },
    {
      id: 'recent-2',
      username: 'Alex_Gaming',
      uuid: 'recent-uuid-2',
      avatar: '/demo-avatars/jeb-head.png',
      type: 'offline',
      hasMinecraft: false,
    },
    {
      id: 'recent-3',
      username: 'SchoolPlayer',
      uuid: 'recent-uuid-3',
      avatar: '/demo-avatars/dinnerbone-head.png',
      type: 'microsoft',
      hasMinecraft: true,
    },
  ]);
  const [showAccountModal, setShowAccountModal] = useState(false);
  const [downloadTasks, setDownloadTasks] = useState<DownloadTask[]>([]);
  /**
   * 侧栏徽章的数字。
   *
   * 统计的是「仍在进行」的任务（排队/下载/校验/安装），而不是只看 downloading：
   * 点击「安装」时任务先进入 queued，若只数 downloading，徽章在排队阶段完全
   * 不出现，用户会以为没生效。
   */
  const activeTaskCount = downloadTasks.filter(task => ACTIVE_TASK_STATUSES.includes(task.status)).length;
  const downloadsLabel = activeTaskCount > 0
    ? t('app.downloadsActive', { values: { count: activeTaskCount } })
    : t('app.downloads');
  const [showSettings, setShowSettings] = useState(false);
  const [javaRuntimes, setJavaRuntimes] = useState<JavaRuntime[]>(loadJavaList);
  const [globalJavaPath, setGlobalJavaPath] = useState(() => {
    try { return JSON.parse(localStorage.getItem('flowlaunch-settings') || '{}').javaPath || ''; } catch { return ''; }
  });
  const [instanceJavaSettings, setInstanceJavaSettings] = useState<Record<string, InstanceJavaSetting>>(loadInstanceJavaSettings);
  /**
   * 全局设置的只读快照。
   *
   * 管理页需要显示「跟随全局时实际是什么值」，但全局设置在 SettingsPanel 里
   * 保存。这里存一份，在设置面板保存后同步，避免管理页读到一个陈旧的值。
   */
  const [globalPrefs, setGlobalPrefs] = useState<LauncherPrefs>(() => readPrefs());
  const [instanceOverrides, setInstanceOverrides] = useState<OverrideMap>(loadInstanceOverrides);
  const [pluginStore, setPluginStore] = useState<PluginStore>(loadPluginStore);
  /** 改过但还没重启的插件（见 PluginWorkspace 的同名 prop）。 */
  const [pendingPluginRestart, setPendingPluginRestart] = useState<string[]>([]);
  /**
   * 侧栏徽章只统计「需要注意」的插件。
   *
   * 用总数当徽章会让它永远非零（内置插件本来就装着），徽章就失去提醒意义。
   * 判定标准与页面里的「需要注意」筛选共用 pluginNeedsAttention，避免徽章说
   * 1、点进去却列出 3 个对不上。
   */
  const attentionPluginCount = allPlugins(pluginStore)
    .filter(plugin => pluginNeedsAttention(plugin, pendingPluginRestart)).length;
  const [showFirstRun, setShowFirstRun] = useState(true);
  const [storeCategory, setStoreCategory] = useState<StoreCategory>('vanilla');
  const [storeMenuOpen, setStoreMenuOpen] = useState(false);
  const [storeMenuPos, setStoreMenuPos] = useState({ top: 0, left: 0, maxHeight: 320 });
  const storeMenuButtonRef = useRef<HTMLButtonElement>(null);
  const storeMenuRef = useRef<HTMLDivElement>(null);

  // 模拟数据
  useEffect(() => {
    // 检查是否首次运行
    const hasVisited = localStorage.getItem('hasVisited');
    if (hasVisited) {
      setShowFirstRun(false);
      
      // 加载模拟数据
      const mockInstances: GameInstance[] = [
        {
          id: '1',
          name: 'Minecraft 1.20.4',
          version: '1.20.4',
          loader: 'vanilla',
          javaVersion: '21',
          lastPlayed: new Date(Date.now() - 1000 * 60 * 60 * 2),
          playTime: 14520,
          modCount: 0,
          status: 'ready'
        },
        {
          id: '2',
          // 演示实例在首次运行时就落库了，名字按当时的语言生成一次；真实实例
          // 由用户创建，不经过这里。
          name: t('app.demoModpack'),
          version: '1.20.1',
          loader: 'fabric',
          loaderVersion: '0.15.3',
          javaVersion: '17',
          lastPlayed: new Date(Date.now() - 1000 * 60 * 60 * 24),
          playTime: 45680,
          modCount: 47,
          status: 'ready'
        }
      ];
      
      setInstances(mockInstances);
      setContext('home');
    }
  }, []);

  const completeFirstRun = () => {
    localStorage.setItem('hasVisited', 'true');
    setShowFirstRun(false);
    setContext('home');
  };

  const createNewInstance = () => {
    setContext('store');
  };

  const queueStoreDownload = (name: string, type: DownloadTask['type'], targetInstance?: string) => {
    const task: DownloadTask = {
      id: `store-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
      name,
      type,
      status: 'queued',
      progress: 0,
      targetInstance,
    };
    setDownloadTasks(current => [task, ...current]);
  };

  const selectStoreInstance = (instanceId: string) => {
    const instance = instances.find(item => item.id === instanceId);
    setSelectedInstance(instance);
  };

  const toggleStoreMenu = () => {
    if (storeMenuOpen) {
      setStoreMenuOpen(false);
      return;
    }
    const rect = storeMenuButtonRef.current?.getBoundingClientRect();
    if (!rect) return;
    const width = 236;
    const left = Math.max(8, Math.min(rect.left, window.innerWidth - width - 8));
    const top = Math.min(rect.bottom + 8, window.innerHeight - 80);
    setStoreMenuPos({ top, left, maxHeight: Math.max(180, window.innerHeight - top - 16) });
    setStoreMenuOpen(true);
  };

  const chooseStoreCategory = (category: StoreCategory) => {
    setStoreCategory(category);
    setContext('store');
    setStoreMenuOpen(false);
  };

  useEffect(() => {
    if (!storeMenuOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!storeMenuRef.current?.contains(target) && !storeMenuButtonRef.current?.contains(target)) setStoreMenuOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setStoreMenuOpen(false);
    };
    const closeOnViewportChange = () => setStoreMenuOpen(false);
    window.addEventListener('pointerdown', onPointerDown);
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('resize', closeOnViewportChange);
    window.addEventListener('scroll', closeOnViewportChange, true);
    return () => {
      window.removeEventListener('pointerdown', onPointerDown);
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('resize', closeOnViewportChange);
      window.removeEventListener('scroll', closeOnViewportChange, true);
    };
  }, [storeMenuOpen]);

  const persistInstanceJava = (instanceId: string, setting: InstanceJavaSetting) => {
    const next = { ...instanceJavaSettings, [instanceId]: setting };
    setInstanceJavaSettings(next);
    saveInstanceJavaSettings(next);
  };

  /**
   * 保存一个实例的覆盖设置。
   *
   * 每次点击都立即落盘，不跟全局设置共用「保存/关闭」流程：管理页里改动一项
   * 就是一次明确的选择，再要求用户按保存容易漏掉。空对象直接删掉该实例的
   * 记录，避免 localStorage 里堆满空条目。
   */
  /**
   * 插件状态立即落盘。
   *
   * 与实例覆盖同理：开关一个插件是一次明确的选择，再要求按保存容易漏掉；
   * 而且插件与全局设置无关，没有「取消」的语义。
   */
  const persistPlugins = (next: PluginStore) => {
    setPluginStore(next);
    savePluginStore(next);
  };

  const persistOverrides = (instanceId: string, next: InstanceOverrides) => {
    const map = { ...instanceOverrides };
    if (Object.keys(next).length === 0) delete map[instanceId];
    else map[instanceId] = next;
    setInstanceOverrides(map);
    saveInstanceOverrides(map);
  };

  const cancelTask = (taskId: string) => {
    setDownloadTasks(current => current.map(task => (
      task.id === taskId ? { ...task, status: 'cancelled' as const } : task
    )));
  };

  const launchInstance = (instance: GameInstance) => {
    alert(t('app.launchAlert', { values: { name: instance.name } }));
  };

  const handleLogin = (account: Account | undefined) => {
    setCurrentAccount(account);
    if (account) {
      // 更新最近使用列表
      setRecentAccounts(prev => {
        const filtered = prev.filter(a => a.id !== account.id);
        return [account, ...filtered].slice(0, 5);
      });
    }
  };

  /**
   * 退出登录。
   *
   * 只清掉当前账户，保留最近账户列表——和多数启动器一致：退出后那台机器上用过
   * 的账户仍可在账户弹窗里一键切回，不必重新走登录流程。已安装的游戏、模组和
   * 存档与账户无关，不受影响。
   */
  const signOut = () => {
    if (!currentAccount) return;
    const name = currentAccount.username;
    if (!window.confirm(t('app.signOutConfirm', { values: { name } }))) return;
    setCurrentAccount(undefined);
  };

  const toggleTheme = () => {
    const newTheme = resolvedTheme === 'light' ? 'dark' : 'light';
    setTheme(newTheme);
  };

  // 首次运行引导
  if (showFirstRun) {
    return (
      <div className={styles.app}>
        <div className={styles.firstRun}>
          <div className={styles.firstRunContent}>
            <div className={styles.logo}>
              <div className={styles.logoIcon}>MFL</div>
            </div>
            <h1 className={styles.firstRunTitle}>{t('app.firstRunTitle')}</h1>
            <p className={styles.firstRunDescription}>
              {t('app.firstRunTagline')}
              <br />
              {t('app.firstRunSubtagline')}
            </p>
            <div className={styles.firstRunActions}>
              <Button size="lg" onClick={completeFirstRun} icon={<PlayIcon />}>
                {t('app.getStarted')}
              </Button>
            </div>
            <div className={styles.firstRunFeatures}>
              <div className={styles.feature}>
                <ShopIcon />
                <span>{t('app.featureInstall')}</span>
              </div>
              <div className={styles.feature}>
                <SettingsIcon />
                <span>{t('app.featureJava')}</span>
              </div>
              <div className={styles.feature}>
                <LibraryIcon />
                <span>{t('app.featureInstances')}</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    );
  }

  // 主界面
  return (
    <div className={styles.app}>
      {/* 侧边栏 */}
      <aside className={styles.sidebar}>
        <div className={styles.sidebarHeader}>
          <div className={styles.logo}>
            <div className={styles.logoIconSmall}>MFL</div>
            <span className={styles.logoText}>MFMC Launcher</span>
          </div>
        </div>

        <nav className={styles.nav}>
          <button
            className={`${styles.navItem} ${context === 'home' ? styles.active : ''}`}
            onClick={() => setContext('home')}
            aria-label={t('app.library')}
            title={t('app.library')}
          >
            <LibraryIcon />
            <span>{t('app.library')}</span>
            {instances.length > 0 && (
              <span className={styles.badge}>{instances.length}</span>
            )}
          </button>
          <div className={styles.storeMenuWrap}>
            <button
              ref={storeMenuButtonRef}
              className={`${styles.navItem} ${context === 'store' ? styles.active : ''} ${storeMenuOpen ? styles.menuOpen : ''}`}
              onClick={toggleStoreMenu}
              aria-label={t('app.store')}
              aria-haspopup="menu"
              aria-expanded={storeMenuOpen}
              title={t('app.store')}
            >
              <ShopIcon />
              <span>{t('app.store')}</span>
              <ChevronDown className={styles.navChevron} size={14} aria-hidden="true" />
            </button>
            {storeMenuOpen && (
              <div
                ref={storeMenuRef}
                className={styles.storeMenu}
                role="menu"
                aria-label={t('app.chooseDownloadTypeLabel')}
                style={{ top: storeMenuPos.top, left: storeMenuPos.left, maxHeight: storeMenuPos.maxHeight }}
              >
                <div className={styles.storeMenuHeader}>{t('app.chooseDownloadType')}</div>
                {storeCategories.map(item => (
                  <button
                    key={item}
                    type="button"
                    role="menuitemradio"
                    aria-checked={storeCategory === item}
                    className={storeCategory === item ? styles.storeMenuItemActive : ''}
                    onClick={() => chooseStoreCategory(item)}
                  >
                    <span className={styles.storeMenuItemIcon}>
                      {item === 'vanilla'
                        ? <BlockIcon {...VANILLA_BLOCK} size={18} />
                        : CATEGORY_ICONS[item]}
                    </span>
                    <span>{t(CATEGORY_KEYS[item])}</span>
                    {storeCategory === item && <Check className={styles.storeMenuCheck} size={15} />}
                  </button>
                ))}
              </div>
            )}
          </div>
          <button
            className={`${styles.navItem} ${context === 'downloads' ? styles.active : ''}`}
            onClick={() => setContext('downloads')}
            aria-label={downloadsLabel}
            title={downloadsLabel}
          >
            <DownloadIcon />
            <span>{t('app.downloads')}</span>
            {activeTaskCount > 0 && (
              <span className={styles.badge}>{activeTaskCount > 99 ? '99+' : activeTaskCount}</span>
            )}
          </button>
          <button
            className={`${styles.navItem} ${context === 'plugins' ? styles.active : ''}`}
            onClick={() => setContext('plugins')}
            aria-label={t('app.plugins')}
            title={t('app.plugins')}
          >
            <PluginsIcon />
            <span>{t('app.plugins')}</span>
            {/*
              徽章只显示「已启用且需要注意」的插件数量，而不是总数：
              侧栏徽章是提醒，不是计数器；全部插件数放在页面里就够了。
            */}
            {attentionPluginCount > 0 && (
              <span className={styles.badge}>{attentionPluginCount}</span>
            )}
          </button>
        </nav>

        <div className={styles.sidebarFooter}>
          <button className={styles.navItem} onClick={toggleTheme} aria-label={t('app.switchTheme')} title={t('app.switchTheme')}>
            {resolvedTheme === 'light' ? <MoonIcon /> : <SunIcon />}
            <span>{resolvedTheme === 'light' ? t('app.themeToDark') : t('app.themeToLight')}</span>
          </button>
          <button
            className={styles.navItem}
            onClick={() => setShowAccountModal(true)}
            aria-label={t('app.account')}
            title={t('app.account')}
          >
            <SidebarAccountAvatar account={currentAccount} />
            <span>{currentAccount ? currentAccount.username : t('app.login')}</span>
            {/*
              已登录时在名字右侧放一个「退出登录」按钮。用 <span role="button"> 而
              不是嵌套 <button>：HTML 不允许 button 套 button，浏览器会把内层元素
              拆到外面，点击区域和键盘焦点都会错位。这里用 role + tabIndex 保留
              键盘可达性，并在 onClick 里阻止冒泡，避免点退出时又打开账户弹窗。
            */}
            {currentAccount && (
              <span
                role="button"
                tabIndex={0}
                className={styles.signOut}
                aria-label={t('app.signOut')}
                title={t('app.signOut')}
                onClick={event => { event.stopPropagation(); signOut(); }}
                onKeyDown={event => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    event.stopPropagation();
                    signOut();
                  }
                }}
              >
                <SignOutIcon />
              </span>
            )}
          </button>
          <button
            className={styles.navItem}
            onClick={() => setShowSettings(true)}
            aria-label={t('app.settings')}
            title={t('app.settings')}
          >
            <SettingsIcon />
            <span>{t('app.settings')}</span>
          </button>
        </div>
      </aside>

      {/* 主内容区 */}
      <main className={styles.main}>
        {/* 空状态 - 无游戏 */}
        {context === 'home' && instances.length === 0 && (
          <div className={styles.emptyState}>
            <div className={styles.emptyStateContent}>
              <LibraryIcon />
              <h2>{t('app.emptyTitle')}</h2>
              <p>{t('app.emptyHint')}</p>
              <Button icon={<PlusIcon />} onClick={createNewInstance}>
                {t('app.createGame')}
              </Button>
            </div>
          </div>
        )}

        {/* 游戏库 - 快速启动中心 */}
        {context === 'home' && instances.length > 0 && (
          <div className={styles.home}>
            <header className={styles.mainHeader}>
              <h1>{t('app.library')}</h1>
              <Button icon={<PlusIcon />} onClick={createNewInstance}>
                {t('app.createGame')}
              </Button>
            </header>

            <div className={styles.instanceGrid}>
              {instances.map(instance => (
                <Card key={instance.id} hoverable padding="lg">
                  <div className={styles.instanceCard}>
                    <div className={styles.instanceIcon}>
                      {instance.loader === 'vanilla' ? '⛏️' : '🎮'}
                    </div>
                    <div className={styles.instanceInfo}>
                      <h3>{instance.name}</h3>
                      <div className={styles.instanceMeta}>
                        <span>{instance.version}</span>
                        {instance.loader !== 'vanilla' && (
                          <>
                            <span>•</span>
                            <span>{instance.loader}</span>
                          </>
                        )}
                        {instance.modCount > 0 && (
                          <>
                            <span>•</span>
                            <span>{t('app.modCount', { count: instance.modCount })}</span>
                          </>
                        )}
                      </div>
                      {instance.lastPlayed && (
                        <div className={styles.lastPlayed}>
                          {t('app.lastPlayed', { values: { time: new Date(instance.lastPlayed).toLocaleString(locale, {
                            month: 'long',
                            day: 'numeric',
                            hour: '2-digit',
                            minute: '2-digit'
                          }) } })}
                        </div>
                      )}
                    </div>
                    <div className={styles.instanceActions}>
                      <Button
                        variant="primary"
                        icon={<PlayIcon />}
                        onClick={() => launchInstance(instance)}
                      >
                        {t('app.launch')}
                      </Button>
                      <Button
                        variant="ghost"
                        onClick={() => {
                          setSelectedInstance(instance);
                          setContext('instance-detail');
                        }}
                      >
                        {t('app.manage')}
                      </Button>
                    </div>
                  </div>
                </Card>
              ))}
            </div>
          </div>
        )}

        {context === 'instance-detail' && selectedInstance && (
          <InstanceSettings
            instance={selectedInstance}
            global={globalPrefs}
            overrides={instanceOverrides[selectedInstance.id] ?? EMPTY_OVERRIDES}
            onChange={next => persistOverrides(selectedInstance.id, next)}
            onBack={() => setContext('home')}
            onLaunch={() => launchInstance(selectedInstance)}
            onOpenGlobal={() => setShowSettings(true)}
            javaRuntimes={javaRuntimes}
            javaSetting={instanceJavaSettings[selectedInstance.id] ?? { ...DEFAULT_INSTANCE_JAVA }}
            globalJavaPath={globalJavaPath}
            onJavaSettingChange={setting => persistInstanceJava(selectedInstance.id, setting)}
            onManageJava={() => setShowSettings(true)}
          />
        )}

        {context === 'store' && (
          <StoreWorkspace
            category={storeCategory}
            instances={instances}
            selectedInstance={selectedInstance}
            downloadTasks={downloadTasks}
            onSelectInstance={selectStoreInstance}
            onQueueDownload={queueStoreDownload}
          />
        )}

        {context === 'plugins' && (
          <PluginWorkspace
            store={pluginStore}
            onChange={persistPlugins}
            pendingRestart={pendingPluginRestart}
            onPendingRestartChange={setPendingPluginRestart}
          />
        )}

        {/* 下载任务 */}
        {context === 'downloads' && (
          <div className={styles.downloads}>
            <header className={styles.mainHeader}>
              <h1>{t('app.downloads')}</h1>
            </header>
            {downloadTasks.length === 0 ? (
              <div className={styles.emptyState}>
                <div className={styles.emptyStateContent}>
                  <DownloadIcon />
                  <h2>{t('app.noDownloadTasks')}</h2>
                  <p>{t('app.noDownloadTasksHint')}</p>
                </div>
              </div>
            ) : (
              <div className={styles.taskList}>
                {downloadTasks.map(task => (
                  <Card key={task.id} padding="md">
                    <div className={styles.taskCard}>
                      <div className={styles.taskInfo}>
                        <h3>{task.name}</h3>
                        <div className={styles.taskProgress}>
                          <div className={styles.progressBar}>
                            <div 
                              className={styles.progressFill}
                              style={{ width: `${task.progress}%` }}
                            />
                          </div>
                          <span>{task.progress}%</span>
                        </div>
                        {/* 状态文字：让侧栏徽章的数字在列表里有对应物。 */}
                        <span className={styles.taskStatus}>{t(TASK_STATUS_KEYS[task.status])}</span>
                      </div>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => cancelTask(task.id)}
                        disabled={!ACTIVE_TASK_STATUSES.includes(task.status)}
                      >
                        {t('app.cancel')}
                      </Button>
                    </div>
                  </Card>
                ))}
              </div>
            )}
          </div>
        )}
      </main>

      <AccountModal
        isOpen={showAccountModal}
        onClose={() => setShowAccountModal(false)}
        currentAccount={currentAccount}
        onLogin={handleLogin}
        recentAccounts={recentAccounts.filter(a => a.id !== currentAccount?.id)}
      />

      {showSettings && <SettingsPanel
        theme={theme}
        onThemeChange={setTheme}
        onClose={() => { setShowSettings(false); setGlobalPrefs(readPrefs()); }}
        instance={context === 'instance-detail' ? selectedInstance : undefined}
        initialTab={context === 'instance-detail' ? 'java' : undefined}
        onJavaRuntimesChange={setJavaRuntimes}
        onGlobalJavaChange={setGlobalJavaPath}
        onInstanceJavaChange={persistInstanceJava}
      />}
    </div>
  );
}

export default App;
