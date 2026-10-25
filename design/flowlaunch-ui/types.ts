// 主题系统
export interface ColorPalette {
  primary: string;
  primaryHover: string;
  primaryActive: string;
  secondary: string;
  success: string;
  warning: string;
  danger: string;
  bgPrimary: string;
  bgSecondary: string;
  bgElevated: string;
  border: string;
  textPrimary: string;
  textSecondary: string;
  textDisabled: string;
}

export interface Theme {
  colors: ColorPalette;
  spacing: Record<string, string>;
  radius: Record<string, string>;
  shadows: Record<string, string>;
}

// 游戏实例
export interface GameInstance {
  id: string;
  name: string;
  version: string;
  loader?: 'forge' | 'neoforge' | 'fabric' | 'quilt' | 'vanilla';
  loaderVersion?: string;
  javaVersion: string;
  lastPlayed?: Date;
  playTime: number;
  modCount: number;
  status: 'ready' | 'installing' | 'updating' | 'error';
  icon?: string;
}

// 模组
export interface Mod {
  id: string;
  name: string;
  slug: string;
  author: string;
  description: string;
  icon?: string;
  downloadCount: number;
  updatedAt: Date;
  versions: ModVersion[];
  source: 'modrinth' | 'curseforge';
  categories: string[];
}

export interface ModVersion {
  id: string;
  versionNumber: string;
  gameVersions: string[];
  loaders: string[];
  dependencies: Dependency[];
  fileSize: number;
  downloads: number;
  datePublished: Date;
}

export interface Dependency {
  modId: string;
  modName: string;
  type: 'required' | 'optional' | 'incompatible';
}

// 账户
export interface Account {
  id: string;
  username: string;
  uuid: string;
  type: 'microsoft' | 'offline';
  avatar?: string;
  hasMinecraft: boolean;
  accessToken?: string;
  refreshToken?: string;
  expiresAt?: Date;
}

// 下载任务
export interface DownloadTask {
  id: string;
  name: string;
  type: 'game' | 'mod' | 'modpack' | 'resource';
  status: 'queued' | 'downloading' | 'verifying' | 'installing' | 'completed' | 'failed' | 'paused' | 'cancelled';
  progress: number;
  total?: number;
  speed?: number;
  error?: string;
  targetInstance?: string;
}

// 应用状态
export type AppContext = 
  | 'first-run'
  | 'empty-library'
  | 'home'
  | 'instance-detail'
  | 'store'
  | 'plugins'
  | 'downloads'
  | 'diagnostics';

export interface AppState {
  context: AppContext;
  selectedInstance?: GameInstance;
  currentAccount?: Account;
  theme: 'light' | 'dark' | 'system';
  instances: GameInstance[];
  downloadTasks: DownloadTask[];
}

// 插件系统
export interface Plugin {
  id: string;
  name: string;
  version: string;
  author: string;
  description: string;
  enabled: boolean;
  hooks: PluginHooks;
}

export interface PluginHooks {
  onInstanceCreate?: (instance: GameInstance) => void | Promise<void>;
  onInstanceLaunch?: (instance: GameInstance) => void | Promise<void>;
  onModInstall?: (mod: Mod, instance: GameInstance) => void | Promise<void>;
  customViews?: CustomView[];
  themeOverrides?: Partial<Theme>;
  backgroundProviders?: BackgroundProvider[];
}

export interface CustomView {
  id: string;
  label: string;
  icon: string;
  position: 'sidebar' | 'main' | 'modal';
  component: React.ComponentType;
}

export interface BackgroundProvider {
  id: string;
  name: string;
  type: 'image' | 'video' | 'shader';
  source: string | (() => string);
}

// 诊断信息
export interface DiagnosticReport {
  timestamp: Date;
  instanceId: string;
  instanceName: string;
  errorType: 'crash' | 'launch-failure' | 'mod-conflict' | 'java-mismatch' | 'memory' | 'unknown';
  severity: 'low' | 'medium' | 'high' | 'critical';
  message: string;
  possibleCauses: string[];
  suggestedFixes: SuggestedFix[];
  logSnippet?: string;
  fullLogPath?: string;
}

export interface SuggestedFix {
  title: string;
  description: string;
  action?: () => void | Promise<void>;
  autoFixable: boolean;
}
