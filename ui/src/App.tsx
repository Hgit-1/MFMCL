import { useEffect, useMemo, useState, type FormEvent } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { listen } from '@tauri-apps/api/event'
import {
  Activity,
  ArrowDownToLine,
  ArrowRight,
  ArrowUpRight,
  Check,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  CircleHelp,
  CloudDownload,
  Cpu,
  Download,
  FileText,
  FolderOpen,
  Gamepad2,
  HardDrive,
  House,
  Layers3,
  LockKeyhole,
  Menu,
  Moon,
  Palette,
  Play,
  Plus,
  Search,
  Settings2,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Sun,
  Terminal,
  X,
} from 'lucide-react'
import { beginLaunch, createInstance, createOfflineAccount, installInstance, inspectJava, isDesktopShell, listAccounts, listInstances, listVersions, loadSettings, readSettings, saveSettings, selectAccount } from './api'
import type { AccountRecord, DownloadEvent, DownloadSource, InstanceRecord, LauncherSettings, PageId, ThemeId, VersionItem } from './types'
import { accessibleAccent } from './theme'
import { InstanceManager } from './components/InstanceManager'
import { InstallationTasks } from './components/InstallationTasks'
import { MicrosoftLoginDialog, SkinLoginDialog } from './components/AccountLoginDialogs'
import './components/account-login.css'
import './components/management.css'
import './App.css'

const initialSettings: LauncherSettings = {
  theme: 'moss',
  customAccent: '#a6d991',
  customBackground: null,
  mode: 'dark',
  downloadSource: 'official',
  ariaConnections: 8,
  ariaSplits: 4,
  showGameLog: true,
  javaPath: '',
  memoryMb: 2048,
}

const themeChoices: { id: ThemeId; label: string; color: string; note: string }[] = [
  { id: 'moss', label: '苔原绿', color: '#a6d991', note: '沉静自然' },
  { id: 'ember', label: '暖焰橙', color: '#f2a773', note: '温暖明亮' },
  { id: 'lavender', label: '雾紫', color: '#b9a5ff', note: '柔和克制' },
  { id: 'ocean', label: '湖水青', color: '#7ed4c1', note: '清爽专注' },
]

const navItems: { id: PageId; label: string; icon: typeof House }[] = [
  { id: 'home', label: '概览', icon: House },
  { id: 'library', label: '实例库', icon: Layers3 },
  { id: 'downloads', label: '下载任务', icon: ArrowDownToLine },
  { id: 'settings', label: '设置', icon: Settings2 },
]

const previewVersions: VersionItem[] = [
  { id: '26.3', type: 'release', releaseTime: '2026-09-15T11:23:02Z' },
  { id: '26.2', type: 'release', releaseTime: '2026-06-16T12:03:33Z' },
  { id: '1.21.11', type: 'release', releaseTime: '2025-12-09T12:23:30Z' },
  { id: '1.21.10', type: 'release', releaseTime: '2025-10-07T09:17:23Z' },
  { id: '1.20.1', type: 'release', releaseTime: '2023-06-12T12:00:00Z' },
]

function App() {
  const [page, setPage] = useState<PageId>('home')
  const [settings, setSettings] = useState<LauncherSettings>(() => readSettings() ?? initialSettings)
  const [settingsLoaded, setSettingsLoaded] = useState(!isDesktopShell())
  const [versions, setVersions] = useState<VersionItem[]>([])
  const [versionSource, setVersionSource] = useState<'loading' | 'official' | 'preview'>(() => isDesktopShell() ? 'loading' : 'preview')
  const [versionLoading, setVersionLoading] = useState(() => isDesktopShell())
  const [dialog, setDialog] = useState<'instance' | 'offline-account' | 'skin-account' | 'microsoft-account' | 'account-picker' | null>(null)
  const [toast, setToast] = useState('')
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [instanceName, setInstanceName] = useState('我的第一个世界')
  const [selectedVersion, setSelectedVersion] = useState('')
  const [search, setSearch] = useState('')
  const [accounts, setAccounts] = useState<AccountRecord[]>([])
  const [instances, setInstances] = useState<InstanceRecord[]>([])
  const [javaInfo, setJavaInfo] = useState<{ path: string; majorVersion: number; versionText: string } | null>(null)
  const [downloadEvents, setDownloadEvents] = useState<DownloadEvent[]>([])
  const [gameLogs, setGameLogs] = useState<Record<string, string>>({})
  const [gameStatus, setGameStatus] = useState<Record<string, { status: string; logPath: string }>>({})
  const [selectedAccount, setSelectedAccount] = useState('')
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [customAccent, setCustomAccent] = useState(settings.customAccent)

  useEffect(() => {
    document.documentElement.dataset.theme = settings.mode
    document.documentElement.dataset.palette = settings.theme
    const media = window.matchMedia('(prefers-color-scheme: light)')
    const applyPalette = () => {
      const palette = accessibleAccent(settings.customAccent, settings.mode === 'light' || (settings.mode === 'system' && media.matches))
      document.documentElement.style.setProperty('--user-accent', palette.fill)
      document.documentElement.style.setProperty('--accent-ink', palette.ink)
      document.documentElement.style.setProperty('--accent-text', palette.text)
    }
    applyPalette()
    media.addEventListener('change', applyPalette)
    return () => media.removeEventListener('change', applyPalette)
  }, [settings.customAccent, settings.mode, settings.theme])

  useEffect(() => {
    document.documentElement.style.setProperty('--custom-background', settings.customBackground ? `url("${settings.customBackground}")` : 'none')
    if (!settingsLoaded) return
    const timer = window.setTimeout(() => {
      void saveSettings(settings, !showAdvanced).catch(() => setToast('设置保存失败；请检查存储空间和权限。'))
    }, 250)
    return () => window.clearTimeout(timer)
  }, [settings, showAdvanced, settingsLoaded])

  useEffect(() => {
    if (!isDesktopShell()) return
    void loadSettings().then(({ settings: stored, beginnerMode }) => {
      setSettings(stored)
      setCustomAccent(stored.customAccent)
      setShowAdvanced(!beginnerMode)
      setSettingsLoaded(true)
    }).catch(() => setToast('无法读取本地设置，已停止自动保存以避免覆盖原配置。'))
  }, [])

  useEffect(() => {
    void listVersions()
      .then((items) => {
        setVersions(items.length ? items : previewVersions)
        setVersionSource(items.length ? 'official' : 'preview')
      })
      .catch(() => {
        setVersions(previewVersions)
        setVersionSource('preview')
      })
      .finally(() => setVersionLoading(false))
    void listInstances().then(setInstances).catch(() => undefined)
    void inspectJava().then(setJavaInfo).catch(() => setJavaInfo(null))
    void listAccounts().then((items) => {
      setAccounts(items)
      setSelectedAccount(items.find((item) => item.selected)?.id ?? '')
    }).catch(() => undefined)
  }, [])

  useEffect(() => {
    if (!isDesktopShell()) return
    let unlistenDownload: (() => void) | undefined
    let unlistenLog: (() => void) | undefined
    let unlistenProcess: (() => void) | undefined
    let cancelled = false
    void Promise.all([
      listen<DownloadEvent>('download-progress', (event) => {
        setDownloadEvents((current) => {
          const next = current.filter((item) => item.artifactId !== event.payload.artifactId)
          return [...next, event.payload].slice(-60)
        })
      }),
      listen<{ instanceId: string; line: string }>('game-log-line', (event) => {
        setGameLogs((current) => ({ ...current, [event.payload.instanceId]: event.payload.line }))
      }),
      listen<{ instanceId: string; status: string; logPath: string }>('game-process', (event) => {
        setGameStatus((current) => ({ ...current, [event.payload.instanceId]: { status: event.payload.status, logPath: event.payload.logPath } }))
        if (event.payload.status === 'crashed') {
          setToast('Minecraft 异常退出；可在“设置 → 高级设置 → 崩溃诊断”中查看摘要或导出日志。')
        }
      }),
    ]).then(([download, log, process]) => {
      if (cancelled) { download(); log(); process() }
      else { unlistenDownload = download; unlistenLog = log; unlistenProcess = process }
    }).catch(() => undefined)
    return () => {
      cancelled = true
      unlistenDownload?.()
      unlistenLog?.()
      unlistenProcess?.()
    }
  }, [])

  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null
      const typing = !!target && (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName))
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault()
        setPage('library')
        window.setTimeout(() => document.getElementById('instance-search')?.focus(), 0)
      } else if (event.key === '/' && !typing && page === 'library') {
        event.preventDefault()
        document.getElementById('instance-search')?.focus()
      }
    }
    window.addEventListener('keydown', handleShortcut)
    return () => window.removeEventListener('keydown', handleShortcut)
  }, [page])

  useEffect(() => {
    if (!toast) return
    const timer = window.setTimeout(() => setToast(''), 3600)
    return () => window.clearTimeout(timer)
  }, [toast])

  const filteredVersions = useMemo(
    () => versions.filter((version) => version.id.toLowerCase().includes(search.toLowerCase())),
    [search, versions],
  )

  const updateSettings = (patch: Partial<LauncherSettings>) =>
    setSettings((current) => ({ ...current, ...patch }))

  const handleCreateInstance = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!selectedVersion) {
      setToast('请选择一个 Minecraft 版本。')
      return
    }
    try {
      const instance = await createInstance(selectedVersion, instanceName.trim() || `Minecraft ${selectedVersion}`)
      setInstances((existing) => [instance, ...existing.filter((item) => item.id !== instance.id)])
      setToast(`实例“${instance.name}”已创建；下一步可安装游戏文件。`)
      setDialog(null)
      setPage('library')
    } catch (error) {
      setToast(error instanceof Error ? error.message : '当前桌面后端尚未连接。')
    }
  }

  const handleLaunch = async (instanceId: string) => {
    const account = accounts.find((item) => item.id === selectedAccount)
    if (!account) {
      setDialog('offline-account')
      return
    }
    let offlineConfirmed = false
    if (account.kind === 'offline') {
      offlineConfirmed = window.confirm(
        '你正在使用离线档案启动。此档案不会验证 Microsoft 游戏授权，通常只能连接允许离线身份的服务器。仍要继续启动吗？',
      )
      if (!offlineConfirmed) return
    }
    try {
      await beginLaunch(instanceId, account.id, offlineConfirmed)
      setToast('启动请求已提交。')
    } catch (error) {
      setToast(error instanceof Error ? error.message : '请检查实例安装状态。')
    }
  }

  const handleStop = async (instanceId: string) => {
    if (!window.confirm('关闭正在运行的 Minecraft 进程？未保存的游戏进度可能会丢失。')) return
    try {
      await invoke('stop_game', { instanceId })
      setToast('已请求关闭游戏进程。')
    } catch (error) {
      setToast(error instanceof Error ? error.message : '无法关闭游戏进程。')
    }
  }

  const handleInstall = async (instance: InstanceRecord) => {
    if (instance.state === 'ready') return
    setDownloadEvents([])
    setPage('downloads')
    setToast(`正在准备安装 ${instance.gameVersion}…`)
    try {
      await installInstance(instance.id)
      setInstances(await listInstances())
      setToast(`“${instance.name}”已安装完成。`)
    } catch (error) {
      setInstances(await listInstances().catch(() => instances))
      setToast(error instanceof Error ? error.message : '安装失败，请重试或查看诊断信息。')
    }
  }

  const addOfflineAccount = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const form = new FormData(event.currentTarget)
    const name = String(form.get('playerName') ?? '').trim()
    if (!/^[A-Za-z0-9_]{3,16}$/.test(name)) {
      setToast('离线档案名称需为 3–16 位英文字母、数字或下划线。')
      return
    }
    try {
      const account = await createOfflineAccount(name)
      setAccounts((existing) => [account, ...existing.filter((item) => item.id !== account.id).map((item) => ({ ...item, selected: false }))])
      setSelectedAccount(account.id)
      setDialog(null)
      setToast('离线档案已添加。启动时仍会再次确认。')
    } catch (error) {
      setToast(error instanceof Error ? error.message : '无法保存离线档案。')
    }
  }

  const handleDiagnostics = async (instanceId?: string) => {
    if (!isDesktopShell()) {
      setToast('崩溃诊断将在桌面应用中读取本地游戏日志。')
      return
    }
    const targetId = instanceId ?? instances[0]?.id
    if (!targetId) {
      setToast('先创建一个实例，之后可以分析该实例的游戏日志。')
      return
    }
    try {
      const summary = await invoke<{ title: string; confidence: string; explanation: string; evidence: string[]; filesFound: number }>('summarize_diagnostics', { instanceId: targetId })
      const confidence = summary.confidence === 'low' ? '低置信度线索' : summary.confidence === 'unknown' ? '无法确定' : summary.confidence === 'none' ? '无日志' : '分析结果'
      const shouldExport = window.confirm(`${summary.title}（${confidence}）\n\n${summary.explanation}\n\n找到 ${summary.filesFound} 个日志文件。要导出脱敏诊断包到你选择的文件夹吗？`)
      if (!shouldExport) return
      const directory = await open({ directory: true, multiple: false, title: '选择诊断包保存文件夹' })
      if (!directory || typeof directory !== 'string') return
      const path = await invoke<string>('export_diagnostics', { instanceId: targetId, destinationDir: directory })
      setToast(`诊断包已导出：${path}`)
    } catch (error) {
      setToast(error instanceof Error ? error.message : '诊断失败；原始日志仍保留在游戏目录。')
    }
  }

  const handleAuthenticated = (account: AccountRecord) => {
    // Display the newly saved profile immediately, then refresh from desktop storage.
    setAccounts((current) => [account, ...current.filter((item) => item.id !== account.id).map((item) => ({ ...item, selected: false }))])
    setSelectedAccount(account.id)
    setDialog(null)
    setToast(`${account.kind === 'microsoft' ? 'Microsoft' : '皮肤站'} 账户“${account.displayName}”已连接。`)
    void listAccounts().then((items) => {
      setAccounts(items)
      setSelectedAccount(items.find((item) => item.selected)?.id ?? account.id)
    }).catch(() => setToast('登录已完成，但账户列表刷新失败；请重新打开应用检查账户。'))
  }

  return (
    <div className="app-shell">
      <aside className={`sidebar ${sidebarOpen ? 'sidebar-open' : ''}`}>
        <div className="brand-row">
          <div className="brand-mark" aria-hidden="true"><span /><span /><span /><span /></div>
          <div className="brand-copy"><strong>MFMCL</strong><span>CRAFT YOUR WORLD</span></div>
          <button className="icon-button sidebar-close" aria-label="关闭导航" onClick={() => setSidebarOpen(false)}><X size={18} /></button>
        </div>

        <div className="sidebar-section-label">工作区</div>
        <nav className="primary-nav" aria-label="主导航">
          {navItems.map(({ id, label, icon: Icon }) => (
            <button key={id} className={`nav-item ${page === id ? 'active' : ''}`} onClick={() => { setPage(id); setSidebarOpen(false) }}>
              <Icon size={18} strokeWidth={1.8} />
              <span>{label}</span>
              {id === 'downloads' && <span className="nav-count">0</span>}
            </button>
          ))}
        </nav>

        <div className="sidebar-divider" />
        <div className="sidebar-section-label">快捷入口</div>
        <button className="nav-item nav-muted" onClick={() => setDialog('instance')}><Plus size={18} strokeWidth={1.8} /><span>创建新实例</span></button>
        <button className="nav-item nav-muted" onClick={() => setPage('library')}><FolderOpen size={18} strokeWidth={1.8} /><span>导入整合包</span></button>

        <div className="sidebar-bottom">
          <div className="sidebar-help-card">
            <div className="help-orbit"><CircleHelp size={18} /></div>
            <div><strong>需要一点帮助？</strong><span>看看新手指南与诊断</span></div>
            <ChevronRight size={15} className="help-chevron" />
          </div>
          <div className="sidebar-footer">
            <div className="avatar avatar-guest">G</div>
            <div className="profile-meta"><strong>{accounts.find((item) => item.id === selectedAccount)?.displayName ?? '访客模式'}</strong><span>{accountKindLabel(accounts.find((item) => item.id === selectedAccount))}</span></div>
            <button className="icon-button profile-more" aria-label="账户选项" onClick={() => setDialog('account-picker')}><ChevronDown size={16} /></button>
          </div>
        </div>
      </aside>

      {sidebarOpen && <button className="sidebar-scrim" aria-label="关闭导航" onClick={() => setSidebarOpen(false)} />}

      <main className="main-shell">
        <header className="topbar">
          <div className="topbar-leading">
            <button className="icon-button menu-button" aria-label="打开导航" onClick={() => setSidebarOpen(true)}><Menu size={20} /></button>
            <div className="breadcrumb"><span>工作区</span><ChevronRight size={14} /><strong>{navItems.find((item) => item.id === page)?.label}</strong></div>
          </div>
          <div className="topbar-actions">
            <button className="search-trigger" onClick={() => { setPage('library'); document.getElementById('instance-search')?.focus() }}><Search size={16} /><span>搜索实例</span><kbd>⌘ K</kbd></button>
            <button className="topbar-icon" aria-label="帮助" onClick={() => setToast('帮助中心即将开放。')}><CircleHelp size={18} /></button>
            <button className="status-pill" onClick={() => setPage('settings')}><span className="status-dot" />本地运行<span className="status-chevron">⌄</span></button>
          </div>
        </header>

        <div className="page-content">
          {page === 'home' && <HomePage instances={instances} javaInfo={javaInfo} gameLogs={gameLogs} gameStatus={gameStatus} showGameLog={settings.showGameLog} onCreate={() => setDialog('instance')} onBrowse={() => setPage('library')} onSettings={() => setPage('settings')} onLaunch={handleLaunch} onStop={handleStop} onInstall={handleInstall} onDiagnostics={(id) => void handleDiagnostics(id)} onAccount={() => setDialog('offline-account')} accountName={accounts.find((item) => item.id === selectedAccount)?.displayName} />}
          {page === 'library' && <LibraryPage instances={instances} search={search} setSearch={setSearch} onCreate={() => setDialog('instance')} versionSource={versionSource} onInstall={handleInstall} onLaunch={handleLaunch} filteredVersions={filteredVersions} versionLoading={versionLoading} onToast={setToast} />}
          {page === 'library' && <InstanceManager instances={instances} onChanged={async () => setInstances(await listInstances())} onNotice={setToast} />}
          {page === 'downloads' && <><DownloadsPage source={settings.downloadSource} progress={downloadEvents} onCreate={() => setDialog('instance')} onToast={setToast} /><InstallationTasks onNotice={setToast} retryableInstances={instances.filter((item) => item.state !== 'ready').map((item) => item.id)} onRetry={(id) => { const target = instances.find((item) => item.id === id); if (target) void handleInstall(target) }} /></>}
          {page === 'settings' && <SettingsPage settings={settings} updateSettings={updateSettings} showAdvanced={showAdvanced} setShowAdvanced={setShowAdvanced} customAccent={customAccent} setCustomAccent={setCustomAccent} onMicrosoftAccount={() => setDialog('microsoft-account')} onSkinAccount={() => setDialog('skin-account')} onOfflineAccount={() => setDialog('offline-account')} onDiagnostics={() => void handleDiagnostics()} onToast={setToast} />}
        </div>

        <footer className="app-footer"><span><span className="footer-status-dot" />系统状态正常</span><span>预览构建 · 0.1.0</span><button onClick={() => setToast('完整版本信息将在 About 页面提供。')}>关于 MFMCL <ArrowUpRight size={12} /></button></footer>
      </main>

      {dialog === 'instance' && <InstanceDialog versions={versions} loading={versionLoading} versionSource={versionSource} name={instanceName} setName={setInstanceName} selectedVersion={selectedVersion} setSelectedVersion={setSelectedVersion} onClose={() => setDialog(null)} onSubmit={handleCreateInstance} />}
      {dialog === 'offline-account' && <OfflineAccountDialog onClose={() => setDialog(null)} onSubmit={addOfflineAccount} onSkin={() => setDialog('skin-account')} onMicrosoft={() => setDialog('microsoft-account')} />}
      {dialog === 'account-picker' && <AccountPickerDialog accounts={accounts} selectedAccount={selectedAccount} onClose={() => setDialog(null)} onAdd={() => setDialog('offline-account')} onSelect={async (accountId) => { try { await selectAccount(accountId); setSelectedAccount(accountId); setAccounts((current) => current.map((account) => ({ ...account, selected: account.id === accountId }))); setDialog(null) } catch (error) { setToast(error instanceof Error ? error.message : '无法切换账户。') } }} />}
      {dialog === 'skin-account' && <SkinLoginDialog onClose={() => setDialog(null)} onAuthenticated={handleAuthenticated} />}
      {dialog === 'microsoft-account' && <MicrosoftLoginDialog onClose={() => setDialog(null)} onAuthenticated={handleAuthenticated} />}
      {toast && <div className="toast-message" role="status"><CircleCheck size={17} /><span>{toast}</span><button aria-label="关闭通知" onClick={() => setToast('')}><X size={15} /></button></div>}
    </div>
  )
}

function PageHeading({ eyebrow, title, description, action }: { eyebrow: string; title: string; description: string; action?: React.ReactNode }) {
  return <div className="page-heading"><div><div className="eyebrow">{eyebrow}</div><h1>{title}</h1><p>{description}</p></div>{action}</div>
}

function HomePage({ instances, javaInfo, gameLogs, gameStatus, showGameLog, onCreate, onBrowse, onSettings, onLaunch, onStop, onInstall, onDiagnostics, onAccount, accountName }: { instances: InstanceRecord[]; javaInfo: { path: string; majorVersion: number; versionText: string } | null; gameLogs: Record<string, string>; gameStatus: Record<string, { status: string; logPath: string }>; showGameLog: boolean; onCreate: () => void; onBrowse: () => void; onSettings: () => void; onLaunch: (instanceId: string) => void; onStop: (instanceId: string) => void; onInstall: (instance: InstanceRecord) => void; onDiagnostics: (instanceId: string) => void; onAccount: () => void; accountName?: string }) {
  const activeInstance = instances[0]
  const activeStatus = activeInstance ? gameStatus[activeInstance.id] : undefined
  return (
    <div className="home-page page-enter">
      <PageHeading eyebrow="你的世界，由此开始" title="欢迎回来。" description="把准备工作交给 MFMCL，留更多时间给你的世界。" />

      <section className="hero-card">
        <div className="hero-glow hero-glow-one" /><div className="hero-glow hero-glow-two" />
        <div className="hero-landscape" aria-hidden="true"><div className="moon-shape" /><div className="landscape-ridge ridge-back" /><div className="landscape-ridge ridge-front" /><div className="pixel-tree tree-one"><i /><i /><i /><b /></div><div className="pixel-tree tree-two"><i /><i /><i /><b /></div></div>
        <div className="hero-content">
          <div className="hero-badge"><span className="badge-dot" />{activeStatus?.status === 'running' ? '游戏正在运行' : activeStatus?.status === 'crashed' ? '游戏已崩溃' : activeInstance ? activeInstance.state === 'ready' ? '可以启动' : activeInstance.state === 'failed' ? '需要修复' : '已创建实例' : '准备好开始'}</div>
          <div className="hero-title-block"><p className="hero-kicker">MINECRAFT JAVA EDITION</p><h2>{activeInstance ? <>{activeInstance.name}<br /><em>{activeInstance.gameVersion}</em></> : <>一片空白，<br /><em>正好开始。</em></>}</h2><p className="hero-description">{activeInstance ? `${activeInstance.loader === 'vanilla' ? '原版' : activeInstance.loader} · ${activeInstance.state === 'ready' ? '游戏文件已安装' : '独立游戏目录已准备好'}` : '先创建一个游戏实例。版本、Java 和安装步骤，我们会陪你一起完成。'}</p></div>
          <div className="hero-actions">{activeInstance ? activeStatus?.status === 'crashed' ? <button className="button-primary" onClick={() => onDiagnostics(activeInstance.id)}><CircleAlert size={16} />查看崩溃摘要<ArrowRight size={16} /></button> : <button className="button-primary" disabled={activeStatus?.status === 'running'} onClick={() => activeInstance.state === 'ready' ? onLaunch(activeInstance.id) : onInstall(activeInstance)}>{activeStatus?.status === 'running' ? <Activity size={16} /> : activeInstance.state === 'ready' ? <Play size={16} fill="currentColor" /> : <CloudDownload size={16} />}{activeStatus?.status === 'running' ? '游戏运行中' : activeInstance.state === 'ready' ? '启动游戏' : activeInstance.state === 'failed' ? '修复实例' : '安装游戏文件'}<ArrowRight size={16} /></button> : <button className="button-primary" onClick={onCreate}><Plus size={17} />创建第一个实例<ArrowRight size={16} /></button>}<button className="button-quiet" onClick={onBrowse}>{activeInstance ? '查看实例库' : '浏览版本'}<ChevronRight size={15} /></button></div>
          {activeInstance && activeStatus?.status === 'running' && <div className="single-game-log"><span>Java 进程已启动，请等待游戏窗口就绪。</span><button className="button-secondary small" onClick={() => onStop(activeInstance.id)}>关闭游戏进程</button></div>}
          {showGameLog && activeInstance && gameLogs[activeInstance.id] && <div className="single-game-log"><Terminal size={13} /><span>最新日志</span><code>{gameLogs[activeInstance.id]}</code></div>}
        </div>
        <div className="hero-note"><Sparkles size={14} /><span>{activeInstance ? '每个实例都拥有独立的模组与世界目录' : '你的世界设置将彼此独立，不会互相覆盖'}</span></div>
      </section>

      <div className="section-row"><div><h2 className="section-title">快速开始</h2><p className="section-caption">常用操作都在这里</p></div><button className="text-link" onClick={onSettings}>查看系统状态<ArrowRight size={14} /></button></div>
      <div className="quick-grid">
        <button className="quick-card" onClick={onCreate}><div className="quick-icon quick-icon-green"><Plus size={18} /></div><div className="quick-copy"><strong>创建游戏实例</strong><span>选择版本与模组加载器</span></div><ArrowUpRight className="quick-arrow" size={16} /></button>
        <button className="quick-card" onClick={onBrowse}><div className="quick-icon quick-icon-sand"><Layers3 size={18} /></div><div className="quick-copy"><strong>浏览版本库</strong><span>查看正式版、快照与历史版本</span></div><ArrowUpRight className="quick-arrow" size={16} /></button>
        <button className="quick-card" onClick={onSettings}><div className="quick-icon quick-icon-blue"><SlidersHorizontal size={18} /></div><div className="quick-copy"><strong>检查启动环境</strong><span>Java、存储空间与下载源</span></div><ArrowUpRight className="quick-arrow" size={16} /></button>
      </div>

      <div className="lower-grid">
        <section className="panel readiness-panel">
          <div className="panel-heading"><div><h2>启动环境</h2><p>桌面环境准备情况</p></div><div className="panel-heading-icon"><Activity size={16} /></div></div>
          <div className="readiness-list">
            <ReadinessRow icon={<MonitorIcon />} label="桌面应用" value={isDesktopShell() ? '已连接' : '界面预览'} ok={isDesktopShell()} />
            <ReadinessRow icon={<Cpu size={16} />} label="Java 运行环境" value={javaInfo ? `Java ${javaInfo.majorVersion}` : isDesktopShell() ? '未检测到' : '桌面检测'} ok={!!javaInfo} />
            <ReadinessRow icon={<HardDrive size={16} />} label="游戏实例" value={instances.length ? `${instances.length} 个实例` : '尚未创建'} ok={instances.length > 0} />
          </div>
          <button className="panel-footer-link" onClick={onSettings}>打开设置与诊断<ArrowRight size={14} /></button>
        </section>
        <section className="panel account-panel">
          <div className="panel-heading"><div><h2>账户</h2><p>管理你的游戏身份</p></div><div className="panel-heading-icon account-icon"><ShieldCheck size={16} /></div></div>
          <div className="account-empty">
            <div className="account-avatar"><Gamepad2 size={19} /></div>
            <div className="account-copy"><strong>{accountName ?? '还没有添加账户'}</strong><span>{accountName ? '启动前会显示账户类型提示' : '添加账户后即可开始启动游戏'}</span></div>
          </div>
          <button className="account-add-button" onClick={onAccount}><Plus size={15} />添加账户</button>
        </section>
      </div>
      <div className="privacy-note"><LockKeyhole size={14} /><span>你的存档保存在本机。MFMCL 不会自动上传世界或诊断日志。</span><button onClick={() => window.alert('诊断导出会由你选择保存位置；不会自动上传。')}>了解隐私设置</button></div>
    </div>
  )
}

function MonitorIcon() { return <div className="monitor-icon"><span /></div> }

function ReadinessRow({ icon, label, value, ok }: { icon: React.ReactNode; label: string; value: string; ok: boolean }) {
  return <div className="readiness-row"><span className={`readiness-icon ${ok ? 'ready' : ''}`}>{icon}</span><span className="readiness-label">{label}</span><span className={`readiness-value ${ok ? 'value-ok' : ''}`}>{ok && <Check size={13} />}{value}</span></div>
}

function LibraryPage({ instances, search, setSearch, onCreate, versionSource, onInstall, onLaunch, filteredVersions, versionLoading, onToast }: { instances: InstanceRecord[]; search: string; setSearch: (value: string) => void; onCreate: () => void; versionSource: 'loading' | 'official' | 'preview'; onInstall: (instance: InstanceRecord) => void; onLaunch: (instanceId: string) => void; filteredVersions: VersionItem[]; versionLoading: boolean; onToast: (message: string) => void }) {
  const visibleInstances = instances.filter((instance) => `${instance.name} ${instance.gameVersion} ${instance.loader}`.toLowerCase().includes(search.toLowerCase()))
  if (instances.length > 0) return <div className="library-page page-enter"><PageHeading eyebrow="游戏管理" title="实例库" description="每个实例都有自己的版本、模组和世界存档。" action={<button className="button-primary compact" onClick={onCreate}><Plus size={16} />新建实例</button>} /><div className="library-toolbar"><div className="library-tabs"><button className="library-tab active">所有实例 <span>{instances.length}</span></button><button className="library-tab" onClick={() => onToast('最近启动筛选将在启动历史接入后开放。')}>最近启动</button><button className="library-tab" onClick={() => onToast('收藏筛选将在实例标签接入后开放。')}>已收藏</button></div><label className="library-search"><Search size={15} /><input id="instance-search" value={search} onChange={(event) => setSearch(event.target.value)} placeholder="搜索实例或版本" /><kbd>/</kbd></label></div>{visibleInstances.length === 0 ? <div className="filtered-empty">没有找到匹配的实例。<button onClick={() => setSearch('')}>清除搜索</button></div> : <div className="instance-grid">{visibleInstances.map((instance) => <article className="instance-card" key={instance.id}><div className="instance-card-top"><div className="instance-version-mark"><Layers3 size={17} /></div><span className={`instance-state state-${instance.state}`}>{instance.state === 'ready' ? '已安装' : instance.state === 'installing' ? '正在安装' : instance.state === 'failed' ? '需要修复' : '待安装'}</span></div><h2>{instance.name}</h2><p className="instance-card-version">Minecraft {instance.gameVersion} <span>·</span> {instance.loader === 'vanilla' ? '原版' : instance.loader}</p><div className="instance-card-path"><FolderOpen size={13} /><span title={instance.gameDir}>{instance.gameDir}</span></div><div className="instance-card-footer"><span><ShieldCheck size={12} />独立目录</span><button className="button-primary compact" onClick={() => instance.state === 'ready' ? onLaunch(instance.id) : onInstall(instance)}>{instance.state === 'ready' ? <Play size={13} fill="currentColor" /> : <CloudDownload size={13} />}{instance.state === 'ready' ? '启动' : instance.state === 'failed' ? '修复' : '安装'}</button></div></article>)}</div>}<div className="version-strip"><div className="version-strip-heading"><div><span className="eyebrow">版本索引</span><h2>最近的正式版本</h2></div><span className="version-live"><span />{versionLoading ? '同步中' : versionSource === 'official' ? '官方元数据' : '预览数据'}</span></div><div className="version-list">{filteredVersions.slice(0, 4).map((version) => <button key={version.id} className="version-chip" onClick={onCreate}><span className="version-check"><Check size={12} /></span><strong>{version.id}</strong><span>{new Date(version.releaseTime).toLocaleDateString('zh-CN')}</span><ArrowUpRight size={13} /></button>)}</div></div></div>

  return (
    <div className="library-page page-enter">
      <PageHeading eyebrow="游戏管理" title="实例库" description="每个实例都有自己的版本、模组和世界存档。" action={<button className="button-primary compact" onClick={onCreate}><Plus size={16} />新建实例</button>} />
      <div className="library-toolbar"><div className="library-tabs"><button className="library-tab active">所有实例 <span>0</span></button><button className="library-tab" onClick={() => onToast('筛选器将在实例数据接入后开放。')}>最近启动</button><button className="library-tab" onClick={() => onToast('收藏筛选将在实例数据接入后开放。')}>已收藏</button></div><label className="library-search"><Search size={15} /><input id="instance-search" value={search} onChange={(event) => setSearch(event.target.value)} placeholder="搜索实例或版本" /><kbd>/</kbd></label></div>
      <div className="empty-library"><div className="empty-art"><div className="empty-sun" /><div className="empty-hill hill-a" /><div className="empty-hill hill-b" /><div className="empty-cube"><span /><i /></div><div className="empty-spark spark-a" /><div className="empty-spark spark-b" /></div><div className="empty-title">这里还没有实例</div><p>创建一个实例，为每个版本或整合包留一片独立空间。<br />你的世界数据会保留在本地，不会互相覆盖。</p><button className="button-primary" onClick={onCreate}><Plus size={16} />创建我的第一个实例</button><button className="empty-import" onClick={() => onToast('导入目录功能会在后续迭代接入。')}><FolderOpen size={15} />导入已有游戏目录</button></div>
      <div className="version-strip"><div className="version-strip-heading"><div><span className="eyebrow">版本索引</span><h2>最近的正式版本</h2></div><span className="version-live"><span />{versionLoading ? '同步中' : versionSource === 'official' ? '官方元数据' : '预览数据'}</span></div><div className="version-list">{filteredVersions.slice(0, 4).map((version) => <button key={version.id} className="version-chip" onClick={onCreate}><span className="version-check"><Check size={12} /></span><strong>{version.id}</strong><span>{new Date(version.releaseTime).toLocaleDateString('zh-CN')}</span><ArrowUpRight size={13} /></button>)}</div></div>
    </div>
  )
}

function DownloadsPage({ source, progress, onCreate, onToast }: { source: DownloadSource; progress: DownloadEvent[]; onCreate: () => void; onToast: (message: string) => void }) {
  const sourceLabel = source === 'official' ? '官方源' : source === 'bmclapi' ? 'BMCLAPI' : '混合源'
  const runningCount = progress.filter((item) => item.stage === 'connecting' || item.stage === 'downloading' || item.stage === 'verifying').length
  return <div className="downloads-page page-enter"><PageHeading eyebrow="任务中心" title="下载任务" description="查看安装进度、下载来源与最近的任务记录。" action={<button className="button-secondary compact" onClick={() => onToast(progress.length ? '活动任务无法清理；完成记录会在后续版本中管理。' : '当前没有可清理的任务。')}><X size={15} />清理已完成</button>} />
    <div className="download-summary-grid"><div className="summary-card"><span>当前任务</span><strong>{runningCount}</strong><div><span className="summary-dot dot-green" />{runningCount ? '正在处理' : '队列空闲'}</div></div><div className="summary-card"><span>下载来源</span><strong className="summary-source">{sourceLabel}</strong><div><ShieldCheck size={13} />校验后安装</div></div><div className="summary-card"><span>下载加速</span><strong className="summary-source">aria2 <small>可选组件</small></strong><div><SlidersHorizontal size={13} />线程数在设置中调整</div></div></div>
    {progress.length === 0 ? <section className="task-empty panel"><div className="task-empty-icon"><CloudDownload size={23} /></div><h2>还没有下载任务</h2><p>创建一个实例后，安装任务会显示在这里。<br />网络中断时可以重试，完成后会验证文件完整性。</p><button className="button-primary" onClick={onCreate}><Plus size={16} />创建安装任务</button></section> : <section className="task-list">{progress.map((item) => { const percent = item.total ? Math.min(100, Math.round(item.received / item.total * 100)) : null; const stageLabel = item.stage === 'completed' ? '已完成' : item.stage === 'failed' ? '失败' : item.stage === 'verifying' ? '正在验证' : item.stage === 'connecting' ? '正在连接' : '正在下载'; return <article className="task-card" key={item.artifactId}><div className="task-card-top"><div className="task-file-icon"><FileText size={15} /></div><div className="task-file-copy"><strong>{item.label}</strong><span>{stageLabel}{item.source ? ` · ${item.source}` : ''}</span></div><span className={`task-percent ${item.stage === 'failed' ? 'task-failed' : ''}`}>{percent === null ? item.stage === 'completed' ? '100%' : '…' : `${percent}%`}</span></div><div className="task-progress-track"><i className={percent === null ? 'indeterminate' : ''} style={percent === null ? undefined : { width: `${percent}%` }} /></div><div className="task-card-bottom"><span>{formatBytes(item.received)}{item.total ? ` / ${formatBytes(item.total)}` : ''}</span><span>{item.stage === 'completed' ? '文件校验通过' : item.stage === 'failed' ? '校验或下载失败' : '下载后将自动校验完整性'}</span></div></article> })}</section>}
    <section className="download-notice"><div className="notice-icon"><ShieldCheck size={16} /></div><p><strong>下载文件会先验证，再写入游戏目录。</strong><span>如果校验失败，MFMCL 会尝试下一个已配置来源，不会将未验证文件用于启动。</span></p><button onClick={() => onToast('下载来源可以在设置中切换。')}>了解详情<ArrowRight size={13} /></button></section>
  </div>
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let value = bytes / 1024
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit += 1 }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[unit]}`
}

function SettingsPage({ settings, updateSettings, showAdvanced, setShowAdvanced, customAccent, setCustomAccent, onMicrosoftAccount, onSkinAccount, onOfflineAccount, onDiagnostics, onToast }: { settings: LauncherSettings; updateSettings: (patch: Partial<LauncherSettings>) => void; showAdvanced: boolean; setShowAdvanced: (value: boolean) => void; customAccent: string; setCustomAccent: (value: string) => void; onMicrosoftAccount: () => void; onSkinAccount: () => void; onOfflineAccount: () => void; onDiagnostics: () => void; onToast: (message: string) => void }) {
  const saveAccent = () => updateSettings({ customAccent })
  const currentTheme = themeChoices.find((theme) => theme.id === settings.theme) ?? themeChoices[0]
  return <div className="settings-page page-enter"><PageHeading eyebrow="偏好设置" title="按你的方式来。" description="常用选项用简单语言呈现，技术选项需要时再展开。" action={<button className={`mode-toggle ${showAdvanced ? 'is-on' : ''}`} onClick={() => setShowAdvanced(!showAdvanced)}><SlidersHorizontal size={15} />{showAdvanced ? '高级设置' : '简易设置'}<span className="toggle-track"><i /></span></button>} />
    <div className="settings-layout"><div className="settings-main">
      <section className="settings-card"><div className="settings-card-heading"><div className="setting-heading-icon icon-appearance"><Palette size={17} /></div><div><h2>外观与颜色</h2><p>选择一个预设，或者调出属于你的颜色。</p></div></div><div className="theme-grid">{themeChoices.map((theme) => <button key={theme.id} className={`theme-option ${settings.theme === theme.id ? 'selected' : ''}`} onClick={() => updateSettings({ theme: theme.id, customAccent: theme.color })}><span className="theme-swatch" style={{ '--swatch': theme.color } as React.CSSProperties}>{settings.theme === theme.id && <Check size={13} />}</span><span className="theme-copy"><strong>{theme.label}</strong><small>{theme.note}</small></span>{settings.theme === theme.id && <span className="theme-selected">已选</span>}</button>)}</div><div className="custom-color-row"><div className="custom-color-label"><strong>自定义强调色</strong><span>按钮、选中态和重点标识</span></div><div className="color-input-wrap"><input aria-label="选择自定义强调色" type="color" value={customAccent} onChange={(event) => { setCustomAccent(event.target.value); updateSettings({ customAccent: event.target.value }) }} /><code>{customAccent.toUpperCase()}</code><button className="color-apply" onClick={saveAccent}>应用</button></div></div><div className="custom-background-row"><div className="custom-color-label"><strong>首页背景图片</strong><span>{settings.customBackground ? '已添加到本地设置，图片仅保存在此设备。' : '支持 PNG、JPEG、WebP，最大 2 MB。'}</span></div><div className="background-actions"><label className="button-secondary small background-picker"><Palette size={13} />选择图片<input type="file" accept="image/png,image/jpeg,image/webp" onChange={(event) => { const file = event.target.files?.[0]; if (!file) return; if (file.size > 2 * 1024 * 1024) { onToast('图片大小不能超过 2 MB。'); event.currentTarget.value = ''; return; } if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type)) { onToast('请选择 PNG、JPEG 或 WebP 图片。'); event.currentTarget.value = ''; return; } const reader = new FileReader(); reader.onload = () => { if (typeof reader.result === 'string') updateSettings({ customBackground: reader.result }); }; reader.onerror = () => onToast('无法读取这张图片。'); reader.readAsDataURL(file); event.currentTarget.value = ''; }} /></label>{settings.customBackground && <button className="button-secondary small" onClick={() => updateSettings({ customBackground: null })}><X size={13} />移除</button>}</div></div><div className="appearance-footer"><span>界面主题</span><div className="segmented-control"><button className={settings.mode === 'dark' ? 'selected' : ''} onClick={() => updateSettings({ mode: 'dark' })}><Moon size={14} />深色</button><button className={settings.mode === 'light' ? 'selected' : ''} onClick={() => updateSettings({ mode: 'light' })}><Sun size={14} />浅色</button><button className={settings.mode === 'system' ? 'selected' : ''} onClick={() => updateSettings({ mode: 'system' })}>跟随系统</button></div></div></section>

      <section className="settings-card"><div className="settings-card-heading"><div className="setting-heading-icon icon-download"><Download size={17} /></div><div><h2>下载与网络</h2><p>选择 Minecraft 文件来源和加速方式。</p></div></div><div className="setting-field"><div className="field-copy"><strong>下载来源</strong><span>混合模式会按文件校验后回退到可用来源。</span></div><div className="source-options"><SourceChoice active={settings.downloadSource === 'official'} title="官方" detail="Mojang 官方服务器" onClick={() => updateSettings({ downloadSource: 'official' })} /><SourceChoice active={settings.downloadSource === 'bmclapi'} title="BMCLAPI" detail="国内镜像" onClick={() => updateSettings({ downloadSource: 'bmclapi' })} /><SourceChoice active={settings.downloadSource === 'mixed'} title="混合" detail="镜像 + 官方回退" onClick={() => updateSettings({ downloadSource: 'mixed' })} /></div></div><div className="setting-divider" /><div className="setting-field aria-field"><div className="field-copy"><strong>下载加速</strong><span>aria2 组件仅用于固定版本发布构建；此开发构建默认使用原生校验下载器</span></div><div className="aria-badge"><span className="aria-dot" />待配置</div></div>{showAdvanced && <div className="advanced-download"><div className="slider-setting"><div><strong>同时下载数</strong><span>{settings.ariaConnections} 个文件任务</span></div><input type="range" min="1" max="16" value={settings.ariaConnections} onChange={(event) => updateSettings({ ariaConnections: Number(event.target.value) })} /><span className="slider-value">{settings.ariaConnections}</span></div><div className="slider-setting"><div><strong>单文件连接数</strong><span>服务器支持时才会分段</span></div><input type="range" min="1" max="16" value={settings.ariaSplits} onChange={(event) => updateSettings({ ariaSplits: Number(event.target.value) })} /><span className="slider-value">{settings.ariaSplits}</span></div></div>}</section>

      <section className="settings-card"><div className="settings-card-heading"><div className="setting-heading-icon icon-account"><ShieldCheck size={17} /></div><div><h2>账户与安全</h2><p>账户令牌会保存在系统凭据库中。</p></div></div><div className="account-setting-row"><div className="account-type-icon microsoft-icon">M</div><div className="field-copy"><strong>Microsoft 账户</strong><span>正版账户登录 · 浏览器授权流程</span></div><button className="button-secondary small" onClick={onMicrosoftAccount}>连接账户<ArrowRight size={13} /></button></div><div className="setting-divider" /><div className="account-setting-row"><div className="account-type-icon offline-icon"><Gamepad2 size={15} /></div><div className="field-copy"><strong>离线档案</strong><span>每次启动前都会再次确认。</span></div><button className="button-secondary small" onClick={onOfflineAccount}>添加档案<Plus size={13} /></button></div><div className="setting-divider" /><div className="account-setting-row"><div className="account-type-icon skin-icon"><Sparkles size={15} /></div><div className="field-copy"><strong>皮肤站账户</strong><span>支持可配置的 Yggdrasil 认证服务。</span></div><button className="button-secondary small" onClick={onSkinAccount}>配置服务<ArrowRight size={13} /></button></div></section>

      {showAdvanced && <section className="settings-card"><div className="settings-card-heading"><div className="setting-heading-icon icon-system"><Cpu size={17} /></div><div><h2>启动与诊断</h2><p>为需要手动调整的用户提供更多控制。</p></div></div><div className="setting-field"><label className="form-label">Java 可执行文件（留空则自动检测）<input value={settings.javaPath} placeholder="/path/to/java 或 C:\\Java\\bin\\java.exe" onChange={e => updateSettings({ javaPath: e.target.value })} /></label><label className="form-label">最大游戏内存（MB）<input type="number" min={512} max={32768} step={256} value={settings.memoryMb} onChange={e => updateSettings({ memoryMb: Number(e.target.value) })} /></label></div><div className="setting-field"><div className="field-copy"><strong>单行游戏日志</strong><span>在启动状态区域显示最近一条 MC 日志。</span></div><button className={`switch ${settings.showGameLog ? 'checked' : ''}`} role="switch" aria-checked={settings.showGameLog} onClick={() => updateSettings({ showGameLog: !settings.showGameLog })}><i /></button></div><div className="setting-divider" /><div className="setting-field"><div className="field-copy"><strong>崩溃诊断</strong><span>摘要、查看原始日志或导出到指定文件夹。</span></div><button className="button-secondary small" onClick={onDiagnostics}><FileText size={13} />打开诊断</button></div></section>}
    </div>
    <aside className="settings-aside"><div className="settings-preview-card"><div className="preview-top"><span className="preview-eyebrow">界面预览</span><span className="preview-dots"><i /><i /><i /></span></div><div className="preview-scene"><div className="preview-sun" /><div className="preview-mountain mountain-one" /><div className="preview-mountain mountain-two" /><div className="preview-pixel-tree"><i /><i /><b /></div></div><div className="preview-actions"><span className="preview-action-icon"><Play size={12} fill="currentColor" /></span><span><strong>开始游戏</strong><small>你的下一个世界</small></span><ArrowRight size={14} /></div><div className="preview-theme-line"><span style={{ background: settings.customAccent }} /><span /><span />{currentTheme.label}</div></div><div className="settings-tip"><div className="tip-icon"><Sparkles size={15} /></div><div><strong>新手也能轻松上手</strong><p>推荐值会自动选择。开启高级设置后，才会显示下载连接数和游戏日志开关。</p></div></div><div className="privacy-card"><LockKeyhole size={15} /><div><strong>默认本地优先</strong><span>账户凭据与游戏存档不会自动上传。</span></div></div></aside></div>
  </div>
}

function SourceChoice({ active, title, detail, onClick }: { active: boolean; title: string; detail: string; onClick: () => void }) {
  return <button className={`source-choice ${active ? 'selected' : ''}`} onClick={onClick}><span className="source-radio">{active && <i />}</span><span><strong>{title}</strong><small>{detail}</small></span></button>
}

function InstanceDialog({ versions, loading, versionSource, name, setName, selectedVersion, setSelectedVersion, onClose, onSubmit }: { versions: VersionItem[]; loading: boolean; versionSource: 'loading' | 'official' | 'preview'; name: string; setName: (value: string) => void; selectedVersion: string; setSelectedVersion: (value: string) => void; onClose: () => void; onSubmit: (event: FormEvent<HTMLFormElement>) => void }) {
  return <Modal title="创建游戏实例" eyebrow="新建实例" onClose={onClose}><form onSubmit={onSubmit} className="dialog-form"><label className="form-label">给实例起个名字<input autoFocus value={name} onChange={(event) => setName(event.target.value)} placeholder="例如：生存世界" maxLength={48} /></label><label className="form-label">Minecraft 版本<select value={selectedVersion} onChange={(event) => setSelectedVersion(event.target.value)} required><option value="">选择正式版、快照或历史版本</option>{versions.map((version) => <option key={version.id} value={version.id}>{version.id} · {version.type === 'release' ? '正式版' : version.type === 'snapshot' ? '快照' : '历史版'}</option>)}</select><span className="input-hint">{loading ? '正在读取 Minecraft 官方版本清单…' : versionSource === 'official' ? '版本清单来自官方元数据。' : '暂时无法连接官方清单，当前显示缓存预览版本。'}</span></label><div className="loader-select-card"><div className="loader-icon"><Layers3 size={17} /></div><div><strong>模组加载器</strong><span>先安装原版，后续可为实例添加加载器。</span></div><span className="loader-tag">Vanilla</span></div><div className="dialog-note"><ShieldCheck size={15} /><span>新实例会使用独立目录，原有世界不会被覆盖。</span></div><div className="dialog-actions"><button type="button" className="button-secondary" onClick={onClose}>取消</button><button type="submit" className="button-primary"><Plus size={15} />创建实例</button></div></form></Modal>
}

function accountKindLabel(account?: AccountRecord): string {
  if (!account) return '尚未登录'
  return account.kind === 'offline' ? '离线档案' : account.kind === 'microsoft' ? 'Microsoft 账户' : '皮肤站账户'
}

function AccountPickerDialog({ accounts, selectedAccount, onClose, onAdd, onSelect }: { accounts: AccountRecord[]; selectedAccount: string; onClose: () => void; onAdd: () => void; onSelect: (id: string) => void }) {
  return <Modal title="选择账户" eyebrow="游戏身份" onClose={onClose}><div className="account-picker-list">{accounts.length ? accounts.map((account) => <button key={account.id} className={`account-picker-item ${account.id === selectedAccount ? 'selected' : ''}`} onClick={() => onSelect(account.id)}><span className="account-picker-avatar">{account.kind === 'offline' ? <Gamepad2 size={15} /> : <ShieldCheck size={15} />}</span><span><strong>{account.displayName}</strong><small>{account.kind === 'offline' ? '离线档案 · 启动时会提示' : account.kind === 'microsoft' ? 'Microsoft 账户' : `皮肤站账户${account.authServer ? ` · ${account.authServer}` : ''}`}</small></span>{account.id === selectedAccount && <Check size={15} />}</button>) : <div className="account-picker-empty">尚未添加账户。离线档案会在每次启动时要求确认。</div>}<button className="account-picker-add" onClick={onAdd}><Plus size={15} />添加其他账户</button></div></Modal>
}

function OfflineAccountDialog({ onClose, onSubmit, onSkin, onMicrosoft }: { onClose: () => void; onSubmit: (event: FormEvent<HTMLFormElement>) => void; onSkin: () => void; onMicrosoft: () => void }) {
  return <Modal title="添加账户" eyebrow="游戏身份" onClose={onClose}><div className="account-dialog-intro"><div className="account-dialog-icon"><Gamepad2 size={20} /></div><p>选择登录方式。离线档案每次启动前都会提醒你，且无法用于需要正版验证的服务器。</p></div><form onSubmit={onSubmit} className="dialog-form"><label className="form-label">离线档案名称<input name="playerName" autoFocus minLength={3} maxLength={16} pattern="[A-Za-z0-9_]{3,16}" placeholder="3–16 位英文字母、数字或下划线" required /></label><div className="warning-box"><CircleAlert size={16} /><span>离线档案不验证 Microsoft 游戏授权。请只用于你有权使用的本地或离线模式环境。</span></div><div className="dialog-actions"><button type="button" className="button-secondary" onClick={onSkin}>使用皮肤站账户</button><button type="submit" className="button-primary">添加离线档案<ArrowRight size={14} /></button></div></form><button className="microsoft-signin-button" onClick={onMicrosoft}><span className="ms-logo">▦</span>使用 Microsoft 账户登录</button></Modal>
}

function Modal({ title, eyebrow, onClose, children }: { title: string; eyebrow: string; onClose: () => void; children: React.ReactNode }) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [onClose])
  return <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose() }}><section className="modal-card" role="dialog" aria-modal="true" aria-label={title}><div className="modal-topline"><span>{eyebrow}</span><button className="icon-button" aria-label="关闭对话框" onClick={onClose}><X size={18} /></button></div><h2>{title}</h2>{children}</section></div>
}

export default App
