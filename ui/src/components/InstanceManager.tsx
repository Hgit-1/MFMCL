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
        <button className="button-secondary" disabled={busy} onClick={() => void run(async () => { if (!window.confirm('导出会包含 mods/config 等本地文件，不包含存档。请确认你有权再分发这些文件，并自行检查配置中的私密信息。继续？')) return; const directory = await open({ directory: true }); if (typeof directory !== 'string') return; const path = await invoke<string>('export_modpack', { instanceId: target.id, directory, rightsConfirmed: true }); onNotice(`已导出到 ${path}`) })}><Upload size={16} />导出整合包</button>
      </section></div>
      {error && <p className="management-error" role="alert"><AlertTriangle size={16} />{error}</p>}
      <button className="text-link" disabled={busy} onClick={() => void run(async () => { setMods(await invoke<Mod[]>('list_mods', { instanceId: target.id })); await onChanged() })}><RefreshCw size={14} />刷新实例状态</button>
    </div> : <p className="empty-management">创建或导入实例后，可在这里管理加载器和模组。</p>)}
    {pack && <Dialog title="导入整合包预览" onClose={() => { if (!busy) setPack(null) }}><div className="pack-preview"><h3>{pack.preview.name}</h3><p>版本 {pack.preview.version} · Minecraft {pack.preview.minecraft}</p><p>加载器：{pack.preview.loader}</p><p>必需下载 {pack.preview.requiredFiles} 个 · 可选 {pack.preview.optionalFiles} 个（默认不安装）</p><p>覆盖文件 {pack.preview.overrideFiles} 个 · 上限估算 {(pack.preview.totalBytes / 1024 / 1024).toFixed(1)} MB</p><div className="warning-box"><AlertTriangle size={16} /><span>整合包可能包含可执行模组，仅导入你信任的来源。导入到全新独立目录；完成后仍需安装游戏文件。</span></div><div className="dialog-actions"><button className="button-secondary" disabled={busy} onClick={() => setPack(null)}>取消</button><button className="button-primary" disabled={busy} onClick={() => void run(async () => { await invoke('import_modpack', { path: pack.path }); setPack(null); await onChanged(); onNotice('整合包内容已导入。请点击实例的“安装”以补齐游戏和加载器。') })}>{busy ? '导入并验证文件…' : '信任并导入'}</button></div>{error && <p role="alert">{error}</p>}</div></Dialog>}
  </section>
}
