import { invoke } from '@tauri-apps/api/core'
import type { AccountRecord, InstanceRecord, LauncherSettings, SkinLoginResult, VersionItem } from './types'

export const isDesktopShell = () => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
export const defaultSettings = (): LauncherSettings => ({ theme: 'moss', customAccent: '#a6d991', customBackground: null, mode: 'dark', downloadSource: 'official', ariaConnections: 4, ariaSplits: 4, showGameLog: true, javaPath: '', memoryMb: 2048, updatePolicy: 'notify' })
export function validateSettings(value: unknown): LauncherSettings {
  const fallback = defaultSettings()
  if (!value || typeof value !== 'object') return fallback
  const v = value as Record<string, unknown>
  const number = (n: unknown, defaultValue: number, max: number, min = 1) => typeof n === 'number' && Number.isInteger(n) && n >= min && n <= max ? n : defaultValue
  return {
    theme: ['moss', 'ember', 'lavender', 'ocean'].includes(String(v.theme)) ? v.theme as LauncherSettings['theme'] : fallback.theme,
    mode: ['light', 'dark', 'system'].includes(String(v.mode)) ? v.mode as LauncherSettings['mode'] : fallback.mode,
    customAccent: typeof v.customAccent === 'string' && /^#[0-9a-f]{6}$/i.test(v.customAccent) ? v.customAccent : fallback.customAccent,
    customBackground: typeof v.customBackground === 'string' && /^data:image\/(png|jpeg|webp);base64,[A-Za-z0-9+/=]+$/.test(v.customBackground) && v.customBackground.length < 3 * 1024 * 1024 ? v.customBackground : null,
    downloadSource: ['official', 'bmclapi', 'mixed'].includes(String(v.downloadSource)) ? v.downloadSource as LauncherSettings['downloadSource'] : fallback.downloadSource,
    ariaConnections: number(v.ariaConnections, 4, 16), ariaSplits: number(v.ariaSplits, 4, 16),
    showGameLog: typeof v.showGameLog === 'boolean' ? v.showGameLog : true,
    javaPath: typeof v.javaPath === 'string' && v.javaPath.length < 4096 ? v.javaPath : '',
    memoryMb: number(v.memoryMb, 2048, 32768, 512),
    updatePolicy: ['notify', 'off'].includes(String(v.updatePolicy)) ? v.updatePolicy as LauncherSettings['updatePolicy'] : fallback.updatePolicy,
  }
}
export function readSettings(): LauncherSettings | null {
  try { const value = localStorage.getItem('mfmcl-settings'); return value ? validateSettings(JSON.parse(value)) : null } catch { return null }
}
export async function loadSettings(): Promise<{ settings: LauncherSettings; beginnerMode: boolean }> {
  if (!isDesktopShell()) return { settings: readSettings() ?? defaultSettings(), beginnerMode: true }
  const stored = await invoke<{ themeMode: string; themePreset: string; customAccent: string; customBackground: string | null; beginnerMode: boolean; showGameLog: boolean; javaPath?: string; memoryMb?: number; updatePolicy?: string; download: { source: string; maxConcurrentDownloads: number; connectionsPerFile: number } }>('get_settings')
  return { settings: validateSettings({ theme: stored.themePreset, customAccent: stored.customAccent, customBackground: stored.customBackground, mode: stored.themeMode, downloadSource: stored.download.source, ariaConnections: stored.download.maxConcurrentDownloads, ariaSplits: stored.download.connectionsPerFile, showGameLog: stored.showGameLog, javaPath: stored.javaPath ?? '', memoryMb: stored.memoryMb ?? 2048, updatePolicy: stored.updatePolicy ?? 'notify' }), beginnerMode: stored.beginnerMode }
}
export async function saveSettings(settings: LauncherSettings, beginnerMode: boolean): Promise<void> {
  if (isDesktopShell()) {
    await invoke('save_settings', { settings: { schemaVersion: 1, themeMode: settings.mode, themePreset: settings.theme, customAccent: settings.customAccent, customBackground: settings.customBackground, beginnerMode, showGameLog: settings.showGameLog, javaPath: settings.javaPath || null, memoryMb: settings.memoryMb, updatePolicy: settings.updatePolicy, download: { source: settings.downloadSource, maxConcurrentDownloads: settings.ariaConnections, connectionsPerFile: settings.ariaSplits, aria2Enabled: true } } })
    // Avoid duplicating multi-megabyte images in the WebView preference cache.
    localStorage.setItem('mfmcl-settings', JSON.stringify({ ...settings, customBackground: null }))
  } else localStorage.setItem('mfmcl-settings', JSON.stringify(settings))
}
export async function listVersions(): Promise<VersionItem[]> { return isDesktopShell() ? invoke('list_versions') : [] }
export async function listInstances(): Promise<InstanceRecord[]> { return isDesktopShell() ? invoke('list_instances') : [] }
export async function listAccounts(): Promise<AccountRecord[]> { return isDesktopShell() ? invoke('list_accounts') : [] }
export async function inspectJava(): Promise<{ path: string; majorVersion: number; versionText: string }> {
  if (!isDesktopShell()) throw new Error('Java 检测需要桌面应用后端。')
  return invoke('detect_java')
}
function requireDesktop() { if (!isDesktopShell()) throw new Error('此操作需要桌面应用；当前仅为界面预览。') }
export async function createInstance(version: string, name: string): Promise<InstanceRecord> { requireDesktop(); return invoke('create_instance', { version, name }) }
export async function installInstance(instanceId: string): Promise<void> { requireDesktop(); await invoke('install_instance', { instanceId }) }
export async function selectAccount(accountId: string): Promise<void> { requireDesktop(); await invoke('select_account', { accountId }) }
export async function removeAccount(accountId: string): Promise<void> { requireDesktop(); await invoke('remove_account', { accountId }) }
export async function createOfflineAccount(playerName: string): Promise<AccountRecord> { requireDesktop(); return invoke('add_offline_account', { playerName }) }
export async function microsoftLogin(clientId: string): Promise<AccountRecord> { requireDesktop(); return invoke('microsoft_login', { clientId: clientId.trim() }) }
export function validateSkinServer(server: string): string {
  try {
    const url = new URL(server.trim())
    if (url.protocol !== 'https:' || !url.hostname || url.username || url.password || url.search || url.hash) throw new Error()
    if (!url.pathname.endsWith('/')) url.pathname += '/'
    return url.href
  } catch { throw new Error('请输入不含账号、查询参数或片段的 HTTPS 认证根地址。') }
}
export async function skinLogin(server: string, username: string, password: string, trusted: boolean): Promise<SkinLoginResult> {
  requireDesktop()
  const root = validateSkinServer(server)
  if (!trusted) throw new Error('请先确认你信任此皮肤站认证地址。')
  return invoke('skin_login', { server: root, username: username.trim(), password, trusted })
}
export async function skinSelectProfile(pendingId: string, profileId: string | null): Promise<AccountRecord> { requireDesktop(); return invoke('skin_select_profile', { pendingId, profileId }) }
export async function beginLaunch(instanceId: string, accountId: string, offlineConfirmed: boolean): Promise<void> { requireDesktop(); await invoke('launch_instance', { instanceId, accountId, offlineConfirmed }) }

export interface LauncherUpdateStatus { current: string; latest: string | null; notes: string; available: boolean; url: string | null; sha256: string | null; size: number | null }
export async function checkLauncherUpdate(): Promise<LauncherUpdateStatus> { requireDesktop(); return invoke('check_launcher_update') }
export async function downloadLauncherUpdate(status: LauncherUpdateStatus, directory: string): Promise<string> {
  requireDesktop()
  if (!status.url || !status.sha256) throw new Error('没有可下载的更新。')
  return invoke<string>('download_launcher_update', { url: status.url, sha256: status.sha256, directory })
}
