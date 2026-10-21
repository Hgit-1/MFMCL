import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { Download, Layers3, PackageOpen, RefreshCw, Upload, AlertTriangle } from 'lucide-react'
import { Dialog } from './Dialog'
import type { InstanceRecord } from '../types'
import { isDesktopShell } from '../api'

type Mod = { fileName: string; enabled: boolean; size: number }
type Loader = { version: string; stable: boolean }
type Preview = { name: string; version: string; minecraft: string; loader: string; requiredFiles: number; optionalFiles: number; totalBytes: number; overrideFiles: number }
type ModUpdate = { fileName: string; projectId: string; installedVersion: string; newVersion: string; newVersionId: string; newFileName: string; url: string; sha1: string; size: number }
// The backend enum serializes as serde's externally-tagged form:
// "up_to_date" | {"unknown_source": null} | {"unsupported": "原因"} | {"update_available": {...}}
type ModUpdateReport = { fileName: string; status: Record<string, unknown> | string }
function normalizeReport(report: ModUpdateReport): { fileName: string } & ({ kind: 'up_to_date' } | { kind: 'unknown' } | { kind: 'unsupported'; reason: string } | { kind: 'available'; update: ModUpdate }) {
  const status = report.status
  if (typeof status === 'string') return { fileName: report.fileName, kind: 'up_to_date' }
  if (status && typeof status === 'object') {
    if ('update_available' in status) return { fileName: report.fileName, kind: 'available', update: status.update_available as ModUpdate }
    if ('unsupported' in status) return { fileName: report.fileName, kind: 'unsupported', reason: String(status.unsupported ?? '') }
    if ('unknown_source' in status) return { fileName: report.fileName, kind: 'unknown' }
  }
  return { fileName: report.fileName, kind: 'up_to_date' }
}
type ClientUpdate = { upToDate: boolean; changedFiles: number }
const message = (error: unknown) => error instanceof Error ? error.message : String(error)

export function InstanceManager({ instances, onChanged, onNotice }: { instances: InstanceRecord[]; onChanged: () => Promise<void>; onNotice: (text: string) => void }) {
  const [expanded, setExpanded] = useState(false)
  const [targetId, setTargetId] = useState('')
  const [kind, setKind] = useState<'fabric' | 'quilt'>('fabric')
  const [loaders, setLoaders] = useState<Loader[]>([])
  const [loaderVersion, setLoaderVersion] = useState('')
  const [mods, setMods] = useState<Mod[]>([])
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [pack, setPack] = useState<{ path: string; preview: Preview } | null>(null)
  const [updateReports, setUpdateReports] = useState<ReturnType<typeof normalizeReport>[] | null>(null)
  const [clientUpdate, setClientUpdate] = useState<ClientUpdate | null>(null)
  const target = instances.find(i => i.id === targetId) ?? instances[0]

  useEffect(() => {
    if (!expanded || !target || !isDesktopShell()) return
    let ignore = false
    void invoke<Loader[]>('loader_versions', { kind, gameVersion: target.gameVersion })
      .then(versions => { if (!ignore) { setLoaders(versions); setLoaderVersion(versions[0]?.version ?? ''); setError('') } })
      .catch(e => { if (!ignore) { setLoaders([]); setLoaderVersion(''); setError(message(e)) } })
      .finally(() => { if (!ignore) setLoading(false) })
    // Local mod management remains available even when the loader metadata service is offline.
    void invoke<Mod[]>('list_mods', { instanceId: target.id })
      .then(files => { if (!ignore) setMods(files) })
      .catch(e => { if (!ignore) { setMods([]); setError(message(e)) } })
    return () => { ignore = true }
  }, [kind, target, expanded])

  const run = async (action: () => Promise<void>) => { setBusy(true); setError(''); try { await action() } catch (e) { setError(message(e)); onNotice(message(e)) } finally { setBusy(false) } }
  // PCL2-style update flow: disclosed hash lookup, strong risk confirmation,
  // verified download, old file kept as .old for recovery.
  const checkUpdates = () => run(async () => {
    setUpdateReports(null)
    const reports = await invoke<ModUpdateReport[]>('check_mod_updates', { instanceId: target.id })
    setUpdateReports(reports.map(normalizeReport))
    const count = reports.filter(r => typeof r.status !== 'string' && r.status && 'update_available' in r.status).length
    onNotice(count ? `发现 ${count} 个可更新的模组。` : '模组均已是最新版本。')
  })
  const applyUpdate = (update: ModUpdate) => run(async () => {
    const proceed = window.confirm(`将 ${update.fileName}（${update.installedVersion}）更新到 ${update.newVersion}？\n\n新版本模组可能不兼容旧存档或其他模组，可能导致游戏崩溃甚至损坏存档。如果你在游玩整合包，请不要自行更新模组。\n\n更新前请先备份存档并查看模组更新日志。旧文件会保留为 .old，出问题可手动改回。`)
    if (!proceed) return
    await invoke<string>('update_mod', { instanceId: target.id, update })
    setMods(await invoke<Mod[]>('list_mods', { instanceId: target.id }))
    setUpdateReports(current => current?.filter(report => report.fileName !== update.fileName) ?? null)
    onNotice(`已更新 ${update.newFileName}；旧文件保留为 ${update.fileName}.old。`)
  })
  const checkClient = () => run(async () => {
    setClientUpdate(null)
    const status = await invoke<ClientUpdate>('check_client_update', { instanceId: target.id })
    setClientUpdate(status)
  })
  const choosePack = async () => {
    if (!isDesktopShell()) { onNotice('整合包导入需要桌面应用。'); return }
    const path = await open({ multiple: false, filters: [{ name: 'Modrinth 整合包', extensions: ['mrpack'] }] })
    if (typeof path !== 'string') return
    const preview = await invoke<Preview>('preview_modpack', { path })
    setPack({ path, preview })
  }
  return <section className="management-section">
    <div className="management-toolbar"><div><h2>模组与整合包</h2><p>Fabric / Quilt 加载器、本地模组与 Modrinth 整合包。</p></div><div className="management-buttons"><button className="button-secondary" disabled={busy} onClick={() => void run(choosePack)}><PackageOpen size={16} />导入整合包</button><button className="button-secondary" aria-expanded={expanded} onClick={() => { setExpanded(!expanded); setLoading(true) }}><Layers3 size={16} />{expanded ? '收起管理' : '管理实例'}</button></div></div>
    {expanded && (target ? <div className="management-panel">
      <label className="form-label">选择实例<select value={target.id} disabled={busy} onChange={e => { setTargetId(e.target.value); setLoading(true) }}>{instances.map(i => <option key={i.id} value={i.id}>{i.name} · {i.gameVersion}</option>)}</select></label>
      <div className="management-grid"><section><h3>加载器</h3><p>当前：{target.loader}。先安装原版，再安装加载器。切换加载器请创建新实例。</p>
        <label className="form-label">加载器类型<select value={kind} disabled={busy} onChange={e => { setKind(e.target.value as 'fabric' | 'quilt'); setLoading(true) }}><option value="fabric">Fabric</option><option value="quilt">Quilt</option></select></label>
        <label className="form-label">可用版本<select value={loaderVersion} disabled={busy || loading} onChange={e => setLoaderVersion(e.target.value)}><option value="">{loading ? '正在读取版本…' : '选择版本'}</option>{loaders.map(l => <option key={l.version} value={l.version}>{l.version}{l.stable ? ' · 稳定版' : ''}</option>)}</select></label>
        <button className="button-primary" disabled={busy || loading || !loaderVersion || target.state !== 'ready' || target.loader !== 'vanilla'} onClick={() => void run(async () => { await invoke('install_loader', { instanceId: target.id, kind, version: loaderVersion }); await onChanged(); onNotice('加载器已安装，游戏文件已校验。') })}><Download size={16} />{busy ? '正在处理…' : '安装加载器'}</button><p className="muted-note">Forge / NeoForge 的安装处理器尚未接入，当前不会尝试执行未知安装脚本。</p>
      </section><section><h3>本地模组</h3><p>模组以你的用户权限执行。禁用不会删除文件，正在运行游戏时无法修改。</p>
        {loading ? <p role="status">正在读取模组…</p> : mods.length ? <ul className="mod-file-list">{mods.map(mod => <li key={mod.fileName}><span title={mod.fileName}>{mod.fileName}<small>{Math.ceil(mod.size / 1024)} KB</small></span><button className="button-secondary" disabled={busy} onClick={() => void run(async () => { await invoke('set_mod_enabled', { instanceId: target.id, fileName: mod.fileName, enabled: !mod.enabled }); setMods(await invoke<Mod[]>('list_mods', { instanceId: target.id })) })}>{mod.enabled ? '禁用' : '启用'}</button></li>)}</ul> : <p className="empty-management">mods 目录中还没有模组。</p>}
        <div className="management-buttons">
          <button className="button-secondary" disabled={busy || !mods.length || (target.loader !== 'fabric' && !target.loader.startsWith('fabric') && target.loader !== 'quilt' && !target.loader.startsWith('quilt'))} title={mods.length ? '检查 Modrinth 上的新版本' : '没有已安装的模组'} onClick={checkUpdates}><RefreshCw size={15} />{busy ? '正在检查…' : '检查模组更新'}</button>
          <button className="button-secondary" disabled={busy} onClick={() => void run(async () => { if (!window.confirm('导出会包含 mods/config 等本地文件，不包含存档。请确认你有权再分发这些文件，并自行检查配置中的私密信息。继续？')) return; const directory = await open({ directory: true }); if (typeof directory !== 'string') return; const path = await invoke<string>('export_modpack', { instanceId: target.id, directory, rightsConfirmed: true }); onNotice(`已导出到 ${path}`) })}><Upload size={15} />导出整合包</button>
        </div>
        <p className="muted-note">检查更新会把已安装模组文件的 SHA-1 哈希发送给 Modrinth 以查询新版本；仅用于本次查询。旧模组更新后会保留为 .old 文件。</p>
        {updateReports && <div className="mod-update-list" aria-live="polite">
          {updateReports.filter(report => report.kind !== 'up_to_date').length === 0 ? <p className="empty-management">所有已识别的模组都是最新版本。</p> : updateReports.filter(report => report.kind !== 'up_to_date').map(report => <div key={report.fileName} className={`mod-update-row ${report.kind === 'available' ? 'is-available' : ''}`}>
            <span className="mod-update-copy" title={report.fileName}>
              <strong>{report.fileName}</strong>
              {report.kind === 'available' && <small>{report.update.installedVersion} → {report.update.newVersion}</small>}
              {report.kind === 'unknown' && <small>未在 Modrinth 找到（可能是本地或 CurseForge 模组），无法自动检查</small>}
              {report.kind === 'unsupported' && <small>{report.reason || '此版本无法自动更新'}</small>}
            </span>
            {report.kind === 'available' && <button className="button-secondary" disabled={busy} onClick={() => void applyUpdate(report.update)}><Download size={14} />更新</button>}
          </div>)}
          {updateReports.some(report => report.kind === 'available') && <button className="button-primary" disabled={busy} onClick={() => { const pending = updateReports.filter(report => report.kind === 'available'); if (!window.confirm(`共 ${pending.length} 个模组可更新。请确认已备份存档；整合包模组请勿自行更新。全部更新？`)) return; void (async () => { for (const report of pending) { if (report.kind === 'available') await applyUpdate(report.update) } })() }}><Download size={15} />全部更新（{updateReports.filter(report => report.kind === 'available').length}）</button>}
        </div>}
      </section></div>
      <section className="client-update-section"><h3>客户端文件更新</h3><p>对照 Mojang 官方清单检查本实例的游戏文件；更新会重新校验并只重新下载有变化的文件，已安装的加载器不受影响。</p>
        <div className="management-buttons">
          <button className="button-secondary" disabled={busy || target.state !== 'ready'} onClick={checkClient}><RefreshCw size={15} />{busy ? '正在检查…' : '检查客户端更新'}</button>
          {clientUpdate && (clientUpdate.upToDate ? <p className="empty-management" role="status">客户端文件与官方清单一致。</p> : <p className="management-error" role="status"><AlertTriangle size={15} />官方文件清单有变化（例如 Mojang 重新发布了该版本）。请使用实例上的“安装/修复”完成更新。</p>)}
        </div>
      </section>
      {error && <p className="management-error" role="alert"><AlertTriangle size={16} />{error}</p>}
      <button className="text-link" disabled={busy} onClick={() => void run(async () => { setMods(await invoke<Mod[]>('list_mods', { instanceId: target.id })); await onChanged() })}><RefreshCw size={14} />刷新实例状态</button>
    </div> : <p className="empty-management">创建或导入实例后，可在这里管理加载器和模组。</p>)}
    {pack && <Dialog title="导入整合包预览" onClose={() => { if (!busy) setPack(null) }}><div className="pack-preview"><h3>{pack.preview.name}</h3><p>版本 {pack.preview.version} · Minecraft {pack.preview.minecraft}</p><p>加载器：{pack.preview.loader}</p><p>必需下载 {pack.preview.requiredFiles} 个 · 可选 {pack.preview.optionalFiles} 个（默认不安装）</p><p>覆盖文件 {pack.preview.overrideFiles} 个 · 上限估算 {(pack.preview.totalBytes / 1024 / 1024).toFixed(1)} MB</p><div className="warning-box"><AlertTriangle size={16} /><span>整合包可能包含可执行模组，仅导入你信任的来源。导入到全新独立目录；完成后仍需安装游戏文件。</span></div><div className="dialog-actions"><button className="button-secondary" disabled={busy} onClick={() => setPack(null)}>取消</button><button className="button-primary" disabled={busy} onClick={() => void run(async () => { await invoke('import_modpack', { path: pack.path }); setPack(null); await onChanged(); onNotice('整合包内容已导入。请点击实例的“安装”以补齐游戏和加载器。') })}>{busy ? '导入并验证文件…' : '信任并导入'}</button></div>{error && <p role="alert">{error}</p>}</div></Dialog>}
  </section>
}
