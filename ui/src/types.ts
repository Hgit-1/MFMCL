export type PageId = 'home' | 'library' | 'downloads' | 'settings'
export type DownloadSource = 'official' | 'bmclapi' | 'mixed'
export type ThemeId = 'moss' | 'ember' | 'lavender' | 'ocean'

export interface VersionItem {
  id: string
  type: 'release' | 'snapshot' | 'old_beta' | 'old_alpha'
  releaseTime: string
}

export interface LauncherSettings {
  theme: ThemeId
  customAccent: string
  customBackground: string | null
  mode: 'dark' | 'light' | 'system'
  downloadSource: DownloadSource
  ariaConnections: number
  ariaSplits: number
  showGameLog: boolean
  javaPath: string
  memoryMb: number
  updatePolicy: 'notify' | 'off'
}

export interface InstanceRecord {
  id: string
  name: string
  gameVersion: string
  loader: string
  instanceDir: string
  gameDir: string
  isolated: boolean
  state: 'not_installed' | 'installing' | 'ready' | 'failed'
  createdAt: string
}

export interface AccountRecord {
  id: string
  displayName: string
  kind: 'microsoft' | 'offline' | 'skin_server'
  authServer?: string | null
  selected: boolean
  createdAt: string
}

// Authentication responses contain public profile metadata only, never credentials.
export interface SkinLoginResult {
  pendingId: string
  profiles: { id: string; name: string }[]
}

export interface DownloadEvent {
  artifactId: string
  label: string
  received: number
  total: number | null
  source: string
  stage: 'connecting' | 'downloading' | 'verifying' | 'completed' | 'failed'
}

export interface DownloadTask {
  id: string
  title: string
  detail: string
  progress: number
  state: 'queued' | 'running' | 'done' | 'failed'
}
