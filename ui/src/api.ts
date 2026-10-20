import { invoke } from '@tauri-apps/api/core'
import type { AccountRecord, InstanceRecord, LauncherSettings, VersionItem } from './types'

export const isDesktopShell = () =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

export async function listVersions(): Promise<VersionItem[]> {
  if (!isDesktopShell()) return []
  return invoke<VersionItem[]>('list_versions')
}

export async function inspectJava(): Promise<{ path: string; majorVersion: number; versionText: string }> {
  if (!isDesktopShell()) throw new Error('Java 检测需要桌面应用后端。')
  return invoke('detect_java')
}

export async function listInstances(): Promise<InstanceRecord[]> {
  if (!isDesktopShell()) return []
  return invoke<InstanceRecord[]>('list_instances')
}

export async function listAccounts(): Promise<AccountRecord[]> {
  if (!isDesktopShell()) return []
  return invoke<AccountRecord[]>('list_accounts')
}

export async function loadSettings(): Promise<{ settings: LauncherSettings; beginnerMode: boolean }> {
  if (!isDesktopShell()) return { settings: readSettings() ?? defaultSettings(), beginnerMode: true }
  const stored = await invoke<{
    themeMode: LauncherSettings['mode']
    themePreset: LauncherSettings['theme']
    customAccent: string
    customBackground: string | null
    beginnerMode: boolean
    showGameLog: boolean
    download: {
      source: LauncherSettings['downloadSource']
      maxConcurrentDownloads: number
      connectionsPerFile: number
    }
  }>('get_settings')
  return {
    settings: {
      theme: stored.themePreset,
      customAccent: stored.customAccent,
      customBackground: stored.customBackground,
      mode: stored.themeMode,
      downloadSource: stored.download.source,
      ariaConnections: stored.download.maxConcurrentDownloads,
      ariaSplits: stored.download.connectionsPerFile,
      showGameLog: stored.showGameLog,
    },
    beginnerMode: stored.beginnerMode,
  }
}

export async function saveSettings(settings: LauncherSettings, beginnerMode: boolean): Promise<void> {
  localStorage.setItem('mfmcl-settings', JSON.stringify(settings))
  if (!isDesktopShell()) return
  await invoke('save_settings', {
    settings: {
      schemaVersion: 1,
      themeMode: settings.mode,
      themePreset: settings.theme,
      customAccent: settings.customAccent,
      customBackground: settings.customBackground,
      beginnerMode,
      showGameLog: settings.showGameLog,
      download: {
        source: settings.downloadSource,
        maxConcurrentDownloads: settings.ariaConnections,
        connectionsPerFile: settings.ariaSplits,
        aria2Enabled: true,
      },
    },
  })
}

export function readSettings(): LauncherSettings | null {
  try {
    const value = localStorage.getItem('mfmcl-settings')
    return value ? (JSON.parse(value) as LauncherSettings) : null
  } catch {
    return null
  }
}

function defaultSettings(): LauncherSettings {
  return {
    theme: 'moss',
    customAccent: '#a6d991',
    customBackground: null,
    mode: 'dark',
    downloadSource: 'official',
    ariaConnections: 8,
    ariaSplits: 4,
    showGameLog: true,
  }
}

export async function createInstance(version: string, name: string): Promise<InstanceRecord> {
  if (!isDesktopShell()) throw new Error('请在桌面应用中创建实例。')
  return invoke<InstanceRecord>('create_instance', { version, name })
}

export async function installInstance(instanceId: string): Promise<void> {
  if (!isDesktopShell()) throw new Error('安装功能需要桌面应用后端。')
  await invoke('install_instance', { instanceId })
}

export async function selectAccount(accountId: string): Promise<void> {
  if (!isDesktopShell()) throw new Error('账户切换需要桌面应用后端。')
  await invoke('select_account', { accountId })
}

export async function createOfflineAccount(playerName: string): Promise<AccountRecord> {
  if (!isDesktopShell()) throw new Error('账户保存需要桌面应用后端。')
  return invoke<AccountRecord>('add_offline_account', { playerName })
}

export async function beginLaunch(instanceId: string, accountId: string, offlineConfirmed: boolean): Promise<void> {
  if (!isDesktopShell()) throw new Error('启动功能需要桌面应用后端。')
  await invoke('launch_instance', { instanceId, accountId, offlineConfirmed })
}
