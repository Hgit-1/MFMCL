import React, { useState, useEffect, useMemo } from 'react';
import { Box, Puzzle, Package, Image as ImageIcon, Map } from 'lucide-react';
import { useTheme } from '../hooks/useTheme';
import { Button } from '../components/Button';
import { Card } from '../components/Card';
import { AccountModal } from '../components/AccountModal';
import { SettingsPanel, type JavaRuntime } from '../components/SettingsPanel';
import type { GameInstance, Account, AppContext, DownloadTask } from '../types';
import styles from './App.module.css';
import '../styles/globals.css';

// 图标组件（简化版）
const PlayIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor"><path d="M6 4l10 6-10 6V4z"/></svg>;
const PlusIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><path d="M10 5v10M5 10h10"/></svg>;
const SettingsIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><circle cx="10" cy="10" r="3"/><path d="M10 1v2m0 14v2M1 10h2m14 0h2m-3.5-6.5l1.4-1.4m-11.8 0l-1.4-1.4m12.6 12.6l1.4 1.4m-11.8 0l-1.4 1.4"/></svg>;
const LibraryIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><rect x="2" y="3" width="16" height="14" rx="2"/><path d="M2 7h16"/></svg>;
const ShopIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><path d="M3 6l3-3h8l3 3v11a2 2 0 01-2 2H5a2 2 0 01-2-2V6z"/><path d="M3 6h14v2a3 3 0 01-6 0 3 3 0 01-6 0V6"/></svg>;
const DownloadIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><path d="M10 3v10m-4-4l4 4 4-4M3 17h14"/></svg>;
const PlayerHeadIcon = () => (
  <svg width="20" height="20" viewBox="0 0 20 20" fill="none" aria-hidden="true" shapeRendering="crispEdges">
    <path d="M4 3h12v2h2v10h-2v2H4v-2H2V5h2V3z" fill="#9b633f" />
    <path d="M4 3h12v3h2v3h-4V7H8v2H4v4H2V5h2V3z" fill="#4a2b24" />
    <path d="M6 9h3v2H6V9zm5 0h3v2h-3V9z" fill="#222b3a" />
    <path d="M8 14h4v1H8v-1z" fill="#7a3f32" />
  </svg>
);
function SidebarAccountAvatar({ account }: { account?: Account }) {
  if (!account?.avatar) return <PlayerHeadIcon />;
  return (
    <img
      className={styles.sidebarAvatar}
      src={account.avatar}
      alt={`${account.username} 的 Minecraft 头颅`}
    />
  );
}

const SunIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><circle cx="10" cy="10" r="4"/><path d="M10 2v2m0 12v2M2 10h2m12 0h2m-2.8-5.2l1.4-1.4m-11.2 0L4 4.8m11.2 11.2l1.4 1.4m-11.2 0L4 15.2"/></svg>;
const MoonIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor"><path d="M17.293 13.293A8 8 0 016.707 2.707a8.001 8.001 0 1010.586 10.586z"/></svg>;

const ChevronDownIcon = () => <svg width="18" height="18" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="M5 7.5l5 5 5-5" /></svg>;
const SearchIcon = () => <svg width="18" height="18" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.8"><circle cx="8.5" cy="8.5" r="5.5" /><path d="M13 13l4 4" /></svg>;
const SlidersIcon = () => <svg width="18" height="18" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="M4 5h12M4 10h12M4 15h12" /><circle cx="8" cy="5" r="1.5" fill="currentColor" /><circle cx="13" cy="10" r="1.5" fill="currentColor" /><circle cx="6" cy="15" r="1.5" fill="currentColor" /></svg>;

type StoreCategory = 'vanilla' | 'mod' | 'modpack' | 'resourcepack' | 'world';
type VersionKind = 'release' | 'snapshot' | 'old' | 'special';

interface StoreItem {
  id: string;
  name: string;
  author: string;
  description: string;
  downloads: string;
  updated: string;
  source: 'Modrinth' | 'CurseForge';
  category: StoreCategory;
  icon: string;
}

const storeCategories: { id: StoreCategory; label: string; icon: React.ReactNode }[] = [
  { id: 'vanilla', label: '原版游戏', icon: <Box size={18} strokeWidth={1.8} /> },
  { id: 'mod', label: 'Mod', icon: <Puzzle size={18} strokeWidth={1.8} /> },
  { id: 'modpack', label: '整合包', icon: <Package size={18} strokeWidth={1.8} /> },
  { id: 'resourcepack', label: '资源包', icon: <ImageIcon size={18} strokeWidth={1.8} /> },
  { id: 'world', label: '地图', icon: <Map size={18} strokeWidth={1.8} /> },
];

const vanillaVersions: { version: string; date: string; kind: VersionKind; label: string; java: string }[] = [
  { version: '1.21.4', date: '2024-12-03', kind: 'release', label: '正式版', java: 'Java 21' },
  { version: '1.21.3', date: '2024-10-23', kind: 'release', label: '正式版', java: 'Java 21' },
  { version: '1.21.1', date: '2024-08-08', kind: 'release', label: '正式版', java: 'Java 21' },
  { version: '1.21', date: '2024-06-13', kind: 'release', label: '正式版', java: 'Java 21' },
  { version: '1.20.6', date: '2024-04-29', kind: 'release', label: '正式版', java: 'Java 21' },
  { version: '1.20.4', date: '2023-12-07', kind: 'release', label: '正式版', java: 'Java 17' },
  { version: '24w46a', date: '2024-11-14', kind: 'snapshot', label: '快照版', java: 'Java 21' },
  { version: '23w51a', date: '2023-12-18', kind: 'snapshot', label: '快照版', java: 'Java 17' },
  { version: 'b1.7.3', date: '2011-07-08', kind: 'old', label: '远古版', java: 'Java 8' },
  { version: 'Alpha 1.2.6', date: '2010-07-03', kind: 'old', label: '远古版', java: 'Java 8' },
  { version: '20w14infinite', date: '2020-04-01', kind: 'special', label: '彩蛋版', java: 'Java 8' },
];

const storeItems: StoreItem[] = [
  { id: 'sodium', name: 'Sodium', author: 'JellySquid', description: '提升游戏帧率和渲染性能的 Fabric 模组。', downloads: '2.8 亿', updated: '2 天前', source: 'Modrinth', category: 'mod', icon: '⚡' },
  { id: 'rei', name: "Roughly Enough Items", author: 'mezz', description: '在游戏中查看物品配方和用途。', downloads: '1.1 亿', updated: '5 天前', source: 'Modrinth', category: 'mod', icon: '📖' },
  { id: 'jei', name: "Just Enough Items", author: 'mezz', description: 'Minecraft 物品和配方查看工具。', downloads: '4.9 亿', updated: '1 周前', source: 'CurseForge', category: 'mod', icon: '🔍' },
  { id: 'lithium', name: 'Lithium', author: 'CaffeineMC', description: '优化游戏逻辑，不改变原版玩法。', downloads: '1.6 亿', updated: '3 天前', source: 'Modrinth', category: 'mod', icon: '🚀' },
  { id: 'fabulously-optimized', name: 'Fabulously Optimized', author: 'Fabulously Optimized', description: '专注性能和易用性的 Fabric 整合包。', downloads: '2,400 万', updated: '4 天前', source: 'Modrinth', category: 'modpack', icon: '✨' },
  { id: 'create-optimized', name: 'Create: Above and Beyond', author: 'simibubi', description: '围绕机械动力展开的完整冒险体验。', downloads: '860 万', updated: '2 周前', source: 'CurseForge', category: 'modpack', icon: '⚙️' },
  { id: 'stay-true', name: 'Stay True', author: 'ooblec', description: '保留原版风格的高质量视觉升级资源包。', downloads: '3,600 万', updated: '1 周前', source: 'CurseForge', category: 'resourcepack', icon: '🌿' },
  { id: 'faithful-32', name: 'Faithful 32x', author: 'Faithful Team', description: '经典的 32x 原版风格资源包。', downloads: '8,900 万', updated: '6 天前', source: 'CurseForge', category: 'resourcepack', icon: '🧱' },
  { id: 'skyblock', name: 'SkyBlock 生存岛', author: 'TheBusyBiscuit', description: '从一座空岛开始的经典生存地图。', downloads: '1,200 万', updated: '3 周前', source: 'CurseForge', category: 'world', icon: '☁️' },
  { id: 'city-project', name: '现代城市计划', author: 'BlockWorks', description: '适合建筑和多人游玩的现代城市地图。', downloads: '420 万', updated: '1 个月前', source: 'Modrinth', category: 'world', icon: '🏙️' },
];

interface StoreWorkspaceProps {
  category: StoreCategory;
  instances: GameInstance[];
  selectedInstance?: GameInstance;
  downloadTasks: DownloadTask[];
  onSelectInstance: (instanceId: string) => void;
  onCategoryChange: (category: StoreCategory) => void;
  onQueueDownload: (name: string, type: DownloadTask['type'], targetInstance?: string) => void;
}

function StoreWorkspace({ category, instances, selectedInstance, downloadTasks, onSelectInstance, onCategoryChange, onQueueDownload }: StoreWorkspaceProps) {
  const [query, setQuery] = useState('');
  const [versionKinds, setVersionKinds] = useState<VersionKind[]>(['release']);
  const [sourceFilters, setSourceFilters] = useState<('Modrinth' | 'CurseForge')[]>(['Modrinth', 'CurseForge']);
  const [targetInstanceId, setTargetInstanceId] = useState(selectedInstance?.id ?? '');
  const [versionSort, setVersionSort] = useState<'newest' | 'oldest'>('newest');
  const [submittedQuery, setSubmittedQuery] = useState('');

  useEffect(() => {
    setTargetInstanceId(selectedInstance?.id ?? '');
  }, [selectedInstance?.id]);

  const visibleVersions = useMemo(() => {
    const versions = vanillaVersions.filter(item => versionKinds.includes(item.kind));
    return versions.sort((a, b) => versionSort === 'newest' ? b.date.localeCompare(a.date) : a.date.localeCompare(b.date));
  }, [versionKinds, versionSort]);
  const visibleItems = useMemo(() => storeItems.filter(item => {
    if (item.category !== category || !sourceFilters.includes(item.source)) return false;
    const text = `${item.name} ${item.author} ${item.description}`.toLowerCase();
    return !submittedQuery.trim() || text.includes(submittedQuery.trim().toLowerCase());
  }), [category, submittedQuery, sourceFilters]);

  const toggleVersionKind = (kind: VersionKind) => {
    setVersionKinds(current => current.includes(kind) ? current.filter(item => item !== kind) : [...current, kind]);
  };

  const toggleSource = (source: 'Modrinth' | 'CurseForge') => {
    setSourceFilters(current => current.includes(source) ? current.filter(item => item !== source) : [...current, source]);
  };

  const resetFilters = () => {
    setQuery('');
    setSubmittedQuery('');
    setSourceFilters(['Modrinth', 'CurseForge']);
    setVersionKinds(['release']);
    setVersionSort('newest');
  };

  const isQueued = (name: string) => downloadTasks.some(task => task.name === name && task.status !== 'cancelled' && task.status !== 'failed');

  return (
    <div className={styles.store}>
      <header className={styles.mainHeader}>
        <div>
          <h1>内容下载</h1>
          <p className={styles.storeHeaderHint}>选择分类、设置条件，然后安装到当前实例</p>
        </div>
      </header>

      <div className={styles.storeWorkspace}>
        <aside className={styles.storeNav} aria-label="内容下载分类">
          <div className={styles.storeNavTitle}>内容下载</div>
          <label className={styles.storeCategoryField} htmlFor="store-category">
            <span className={styles.storeCategoryIcon}>{storeCategories.find(item => item.id === category)?.icon}</span>
            <select id="store-category" value={category} onChange={(event) => onCategoryChange(event.target.value as StoreCategory)}>
              {storeCategories.map(item => <option key={item.id} value={item.id}>{item.label}</option>)}
            </select>
          </label>
          <p className={styles.storeCategoryHint}>切换要下载的内容类型</p>
          <div className={styles.storeNavDivider} />
          <div className={styles.storeTargetBlock}>
            <label htmlFor="store-target">当前实例</label>
            <select id="store-target" value={targetInstanceId} onChange={(event) => onSelectInstance(event.target.value)}>
              <option value="">下载任务</option>
              {instances.map(instance => <option key={instance.id} value={instance.id}>{instance.name}</option>)}
            </select>
          </div>
        </aside>

        <section className={styles.storePanel}>
          <div className={styles.storeFilterCard}>
            <div className={styles.storeFilterTitle}>
              <h2>搜索{storeCategories.find(item => item.id === category)?.label}</h2>
              <span>{category === 'vanilla' ? `${visibleVersions.length} 个版本` : `${visibleItems.length} 个结果`}</span>
            </div>
            <div className={styles.storeFilterGrid}>
              {category !== 'vanilla' && (
                <label className={`${styles.storeField} ${styles.storeFieldWide}`}>
                  <span>名称</span>
                  <div className={styles.storeFieldInput}>
                    <SearchIcon />
                    <input value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => event.key === 'Enter' && setSubmittedQuery(query)} placeholder={`输入${storeCategories.find(item => item.id === category)?.label}名称`} />
                  </div>
                </label>
              )}
              {category === 'vanilla' ? (
                <div className={`${styles.storeField} ${styles.storeFieldWide}`}>
                  <span>版本类型</span>
                  <div className={styles.storeCheckGroup}>
                    {([['release', '普通版'], ['snapshot', '快照版'], ['old', '远古版'], ['special', '彩蛋版']] as [VersionKind, string][]).map(([kind, label]) => (
                      <label key={kind} className={styles.checkOption}>
                        <input type="checkbox" checked={versionKinds.includes(kind)} onChange={() => toggleVersionKind(kind)} />
                        <span>{label}</span>
                      </label>
                    ))}
                  </div>
                </div>
              ) : (
                <div className={styles.storeField}>
                  <span>来源</span>
                  <div className={styles.storeCheckGroup}>
                    {(['Modrinth', 'CurseForge'] as const).map(source => (
                      <label key={source} className={styles.checkOption}>
                        <input type="checkbox" checked={sourceFilters.includes(source)} onChange={() => toggleSource(source)} />
                        <span>{source}</span>
                      </label>
                    ))}
                  </div>
                </div>
              )}
              {category === 'vanilla' ? (
                <label className={styles.storeField}>
                  <span>排序</span>
                  <select value={versionSort} onChange={(event) => setVersionSort(event.target.value as 'newest' | 'oldest')}>
                    <option value="newest">从新到旧</option>
                    <option value="oldest">从旧到新</option>
                  </select>
                </label>
              ) : (
                <label className={styles.storeField}>
                  <span>游戏版本</span>
                  <select defaultValue="all">
                    <option value="all">全部版本</option>
                    <option value="1.21.4">1.21.4</option>
                    <option value="1.20.4">1.20.4</option>
                    <option value="1.20.1">1.20.1</option>
                    <option value="1.19.2">1.19.2</option>
                  </select>
                </label>
              )}
            </div>
            <div className={styles.storeFilterActions}>
              <Button onClick={() => setSubmittedQuery(query)} icon={<SearchIcon />}>搜索</Button>
              <Button variant="ghost" onClick={resetFilters}>重置条件</Button>
            </div>
          </div>

          <div className={styles.storeResultsCard}>
            <div className={styles.storeResultsHeader}>
              <h2>{category === 'vanilla' ? '版本列表' : '搜索结果'}</h2>
              <span>{targetInstanceId ? '将安装到已选实例' : '将加入下载任务'}</span>
            </div>
            {category === 'vanilla' ? (
              <div className={styles.versionList}>
                {visibleVersions.length === 0 ? <div className={styles.storeEmpty}>至少选择一种版本类型</div> : visibleVersions.map(item => (
                  <div key={`${item.kind}-${item.version}`} className={styles.versionRow}>
                    <div className={styles.versionMark}>◈</div>
                    <div className={styles.versionMain}>
                      <strong>Minecraft {item.version}</strong>
                      <span>{item.label} · {item.date} · {item.java}</span>
                    </div>
                    <Button size="sm" onClick={() => onQueueDownload(`Minecraft ${item.version}`, 'game', targetInstanceId || undefined)} disabled={isQueued(`Minecraft ${item.version}`)}>
                      {isQueued(`Minecraft ${item.version}`) ? '已加入' : '安装'}
                    </Button>
                  </div>
                ))}
              </div>
            ) : (
              <div className={styles.contentList}>
                {visibleItems.length === 0 ? <div className={styles.storeEmpty}>没有找到匹配内容</div> : visibleItems.map(item => {
                  const taskType: DownloadTask['type'] = category === 'mod' ? 'mod' : category === 'modpack' ? 'modpack' : 'resource';
                  return (
                    <div key={item.id} className={styles.contentRow}>
                      <div className={styles.contentIcon}>{item.icon}</div>
                      <div className={styles.contentMain}>
                        <div className={styles.contentTitleLine}><strong>{item.name}</strong><span className={styles.sourceBadge}>{item.source}</span></div>
                        <p>{item.description}</p>
                        <span className={styles.contentMeta}>{item.author} · {item.downloads} 次下载 · 更新于 {item.updated}</span>
                      </div>
                      <Button size="sm" onClick={() => onQueueDownload(item.name, taskType, targetInstanceId || undefined)} disabled={isQueued(item.name)}>
                        {isQueued(item.name) ? '已加入' : '安装'}
                      </Button>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}

function App() {
  const { theme, setTheme, resolvedTheme } = useTheme();
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
  const [showSettings, setShowSettings] = useState(false);
  const [javaRuntimes, setJavaRuntimes] = useState<JavaRuntime[]>(() => {
    try { return JSON.parse(localStorage.getItem('flowlaunch-java-runtimes') || '[]'); } catch { return []; }
  });
  const [globalJavaPath, setGlobalJavaPath] = useState(() => {
    try { return JSON.parse(localStorage.getItem('flowlaunch-settings') || '{}').javaPath || ''; } catch { return ''; }
  });
  const [instanceJavaPaths, setInstanceJavaPaths] = useState<Record<string, string>>(() => {
    try { return JSON.parse(localStorage.getItem('flowlaunch-instance-java') || '{}'); } catch { return {}; }
  });
  const [showFirstRun, setShowFirstRun] = useState(true);
  const [storeCategory, setStoreCategory] = useState<StoreCategory>('vanilla');

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
          name: '我的模组整合',
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

  const persistInstanceJava = (instanceId: string, path: string) => {
    const next = { ...instanceJavaPaths };
    if (path) next[instanceId] = path; else delete next[instanceId];
    setInstanceJavaPaths(next);
    localStorage.setItem('flowlaunch-instance-java', JSON.stringify(next));
  };

  const launchInstance = (instance: GameInstance) => {
    alert(`启动: ${instance.name}`);
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
              <div className={styles.logoIcon}>FL</div>
            </div>
            <h1 className={styles.firstRunTitle}>欢迎使用 FlowLaunch</h1>
            <p className={styles.firstRunDescription}>
              一个流畅、智能的 Minecraft 启动器
              <br />
              自动管理 Java、模组和资源，让你专注于游戏
            </p>
            <div className={styles.firstRunActions}>
              <Button size="lg" onClick={completeFirstRun} icon={<PlayIcon />}>
                开始使用
              </Button>
            </div>
            <div className={styles.firstRunFeatures}>
              <div className={styles.feature}>
                <ShopIcon />
                <span>一键安装原版与整合包</span>
              </div>
              <div className={styles.feature}>
                <SettingsIcon />
                <span>自动配置 Java 环境</span>
              </div>
              <div className={styles.feature}>
                <LibraryIcon />
                <span>多实例独立管理</span>
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
            <div className={styles.logoIconSmall}>FL</div>
            <span className={styles.logoText}>FlowLaunch</span>
          </div>
        </div>

        <nav className={styles.nav}>
          <button
            className={`${styles.navItem} ${context === 'home' ? styles.active : ''}`}
            onClick={() => setContext('home')}
            aria-label="游戏库"
            title="游戏库"
          >
            <LibraryIcon />
            <span>游戏库</span>
            {instances.length > 0 && (
              <span className={styles.badge}>{instances.length}</span>
            )}
          </button>
          <button
            className={`${styles.navItem} ${context === 'store' ? styles.active : ''}`}
            onClick={() => { setContext('store'); setStoreCategory('vanilla'); }}
            aria-label="内容商店"
            title="内容商店"
          >
            <ShopIcon />
            <span>内容商店</span>
          </button>
          <button
            className={`${styles.navItem} ${context === 'downloads' ? styles.active : ''}`}
            onClick={() => setContext('downloads')}
            aria-label="下载任务"
            title="下载任务"
          >
            <DownloadIcon />
            <span>下载任务</span>
            {downloadTasks.filter(t => t.status === 'downloading').length > 0 && (
              <span className={styles.badge}>
                {downloadTasks.filter(t => t.status === 'downloading').length}
              </span>
            )}
          </button>
        </nav>

        <div className={styles.sidebarFooter}>
          <button className={styles.navItem} onClick={toggleTheme} aria-label="切换主题" title="切换主题">
            {resolvedTheme === 'light' ? <MoonIcon /> : <SunIcon />}
            <span>{resolvedTheme === 'light' ? '深色' : '浅色'}</span>
          </button>
          <button
            className={styles.navItem}
            onClick={() => setShowAccountModal(true)}
            aria-label="账户管理"
            title="账户管理"
          >
            <SidebarAccountAvatar account={currentAccount} />
            <span>{currentAccount ? currentAccount.username : '登录'}</span>
          </button>
          <button
            className={styles.navItem}
            onClick={() => setShowSettings(true)}
            aria-label="设置"
            title="设置"
          >
            <SettingsIcon />
            <span>设置</span>
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
              <h2>还没有游戏</h2>
              <p>开始安装你的第一个 Minecraft</p>
              <Button icon={<PlusIcon />} onClick={createNewInstance}>
                创建新游戏
              </Button>
            </div>
          </div>
        )}

        {/* 游戏库 - 快速启动中心 */}
        {context === 'home' && instances.length > 0 && (
          <div className={styles.home}>
            <header className={styles.mainHeader}>
              <h1>游戏库</h1>
              <Button icon={<PlusIcon />} onClick={createNewInstance}>
                创建新游戏
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
                            <span>{instance.modCount} 个模组</span>
                          </>
                        )}
                      </div>
                      {instance.lastPlayed && (
                        <div className={styles.lastPlayed}>
                          上次游玩: {new Date(instance.lastPlayed).toLocaleString('zh-CN', {
                            month: 'long',
                            day: 'numeric',
                            hour: '2-digit',
                            minute: '2-digit'
                          })}
                        </div>
                      )}
                    </div>
                    <div className={styles.instanceActions}>
                      <Button
                        variant="primary"
                        icon={<PlayIcon />}
                        onClick={() => launchInstance(instance)}
                      >
                        启动
                      </Button>
                      <Button
                        variant="ghost"
                        onClick={() => {
                          setSelectedInstance(instance);
                          setContext('instance-detail');
                        }}
                      >
                        管理
                      </Button>
                    </div>
                  </div>
                </Card>
              ))}
            </div>
          </div>
        )}

        {context === 'instance-detail' && selectedInstance && (
          <div className={styles.instanceDetail}>
            <header className={styles.mainHeader}>
              <div>
                <button className={styles.detailBack} onClick={() => setContext('home')}>← 返回游戏库</button>
                <span className={styles.detailEyebrow}>实例管理</span>
                <h1>{selectedInstance.name}</h1>
                <p>Minecraft {selectedInstance.version}{selectedInstance.loader && selectedInstance.loader !== 'vanilla' ? ` · ${selectedInstance.loader}` : ' · 原版'}</p>
              </div>
              <Button variant="ghost" onClick={() => { setShowSettings(true); }}>设置</Button>
            </header>
            <section className={styles.javaInstanceCard}>
              <div className={styles.javaInstanceHeading}>
                <span className={styles.javaInstanceIcon}>☕</span>
                <div><h2>Java 运行环境</h2><p>为这个实例单独选择 Java，或跟随全局设置。</p></div>
                <span className={styles.instanceOnlyBadge}>实例设置</span>
              </div>
              <label className={styles.javaInstanceField}>
                <span>此实例使用</span>
                <select aria-label={`${selectedInstance.name} 的 Java`} value={instanceJavaPaths[selectedInstance.id] ?? ''} onChange={event => persistInstanceJava(selectedInstance.id, event.target.value)}>
                  <option value="">跟随全局设置{globalJavaPath ? ' · 全局指定' : ' · 按版本自动匹配'}</option>
                  {globalJavaPath && !javaRuntimes.some(runtime => runtime.path === globalJavaPath) && <option value={globalJavaPath}>全局 Java · {globalJavaPath}</option>}
                  {javaRuntimes.map(runtime => <option key={runtime.path} value={runtime.path}>Java {runtime.majorVersion} · {runtime.path}</option>)}
                </select>
              </label>
              <div className={styles.javaInstanceStatus}>
                <span className={styles.statusDot} />
                <div><strong>{instanceJavaPaths[selectedInstance.id] ? '正在使用实例专属 Java' : globalJavaPath ? '正在继承全局 Java 设置' : `将按 Minecraft ${selectedInstance.version} 自动选择兼容版本`}</strong>
                  <small>{instanceJavaPaths[selectedInstance.id] || globalJavaPath || `预计使用 Java ${selectedInstance.javaVersion}`}</small>
                </div>
              </div>
              <div className={styles.javaInstanceActions}>
                <span>优先使用兼容 Minecraft {selectedInstance.version} 的 Java 版本。</span>
                <Button variant="ghost" size="sm" onClick={() => setShowSettings(true)}>管理 Java 列表</Button>
              </div>
            </section>
          </div>
        )}

        {context === 'store' && (
          <StoreWorkspace
            category={storeCategory}
            instances={instances}
            selectedInstance={selectedInstance}
            downloadTasks={downloadTasks}
            onSelectInstance={selectStoreInstance}
            onCategoryChange={setStoreCategory}
            onQueueDownload={queueStoreDownload}
          />
        )}

        {/* 下载任务 */}
        {context === 'downloads' && (
          <div className={styles.downloads}>
            <header className={styles.mainHeader}>
              <h1>下载任务</h1>
            </header>
            {downloadTasks.length === 0 ? (
              <div className={styles.emptyState}>
                <div className={styles.emptyStateContent}>
                  <DownloadIcon />
                  <h2>暂无下载任务</h2>
                  <p>游戏和模组的下载进度会显示在这里</p>
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
                      </div>
                      <Button variant="ghost" size="sm">取消</Button>
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
        onClose={() => setShowSettings(false)}
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
