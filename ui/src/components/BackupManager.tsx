import { useState, type FormEvent } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { Archive, ArchiveRestore, ShieldCheck } from 'lucide-react'
import { Dialog } from './Dialog'
import { isDesktopShell } from '../api'
import type { InstanceRecord } from '../types'

type Preview = { name: string; minecraft: string; loader: string; createdAt: string; fileCount: number; totalBytes: number; archiveSha256: string }
const message = (error: unknown) => error instanceof Error ? error.message : String(error)
export function BackupManager({ instances, onChanged, onNotice }: { instances: InstanceRecord[]; onChanged: () => Promise<void>; onNotice: (text: string) => void }) {
  const [targetId, setTargetId] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [candidate, setCandidate] = useState<{ path: string; preview: Preview } | null>(null)
  const [name, setName] = useState('')
  const target = instances.find(i => i.id === targetId) ?? instances[0]
  const run = async (action: () => Promise<void>) => {
    if (!isDesktopShell()) { onNotice('备份与恢复需要桌面应用。'); return }
    setBusy(true); setError('')
    try { await action() } catch (e) { setError(message(e)) } finally { setBusy(false) }
  }
  const backup = async () => {
    if (!target || !window.confirm('备份包含存档、模组、配置、截图及服务器列表，可能包含私人信息。文件仅保存到你选择的位置，不会上传，也不会加密。请勿随意分享。继续？')) return
    const directory = await open({ directory: true, title: '选择游戏目录之外的备份位置' })
    if (typeof directory !== 'string') return
    const path = await invoke<string>('create_backup', { instanceId: target.id, directory, privacyConfirmed: true })
    onNotice(`本地备份已保存：${path}`)
  }
  const choose = async () => {
    const path = await open({ multiple: false, title: '选择 MFMCL 备份', filters: [{ name: 'MFMCL 私人备份', extensions: ['mfmclbackup'] }] })
    if (typeof path !== 'string') return
    const preview = await invoke<Preview>('preview_backup', { path })
    setCandidate({ path, preview }); setName(`${preview.name.slice(0, 40)}（恢复）`)
  }
  const restore = (event: FormEvent) => {
    event.preventDefault()
    if (!candidate) return
    void run(async () => {
      await invoke('restore_backup', { path: candidate.path, archiveSha256: candidate.preview.archiveSha256, name: name.trim(), confirmed: true })
      setCandidate(null); await onChanged(); onNotice('备份已恢复为新实例；原实例未修改。请安装游戏文件以补齐运行环境和加载器。')
    })
  }
  return <section className="management-section" aria-label="私人备份与恢复">
    <div className="management-toolbar"><div><h2>私人备份与恢复</h2><p>保留世界和游戏配置。恢复始终新建实例，不覆盖原来的世界。</p></div><div className="management-buttons"><button className="button-secondary" disabled={busy || !target} onClick={() => void run(backup)}><Archive size={16} />{busy ? '正在处理…' : '创建本地备份'}</button><button className="button-secondary" disabled={busy} onClick={() => void run(choose)}><ArchiveRestore size={16} />从备份恢复</button></div></div>
    {target && <label className="form-label">要备份的实例<select value={target.id} disabled={busy} onChange={e => setTargetId(e.target.value)}>{instances.map(i => <option key={i.id} value={i.id}>{i.name} · {i.gameVersion}</option>)}</select></label>}
    <p className="muted-note">备份包含 saves、mods、config、资源包、截图和选项；不包含日志、账户凭据或可重新下载的游戏运行库。游戏、安装或模组编辑任务运行时无法备份。单文件上限 2 GB，总计 8 GB。</p>
    {error && <p className="management-error" role="alert">{error}</p>}
    {candidate && <Dialog title="恢复备份为新实例" onClose={() => setCandidate(null)} dismissible={!busy}><form onSubmit={restore} className="dialog-form pack-preview"><p>{candidate.preview.name} · Minecraft {candidate.preview.minecraft}</p><p>加载器：{candidate.preview.loader} · {candidate.preview.fileCount} 个文件 · {(candidate.preview.totalBytes / 1048576).toFixed(1)} MB</p><div className="warning-box neutral"><ShieldCheck size={16} /><span>仅恢复你信任的备份。归档和文件会校验 SHA-256；模组在之后启动游戏时仍会以你的用户权限执行。原实例保持不变。</span></div><label className="form-label">新实例名称<input value={name} onChange={e => setName(e.target.value)} maxLength={48} required disabled={busy} /></label>{error && <p className="management-error" role="alert">{error}</p>}<div className="dialog-actions"><button className="button-secondary" type="button" onClick={() => setCandidate(null)} disabled={busy}>取消</button><button className="button-primary" type="submit" disabled={busy || !name.trim()}>{busy ? '正在校验并恢复…' : '信任并恢复为新实例'}</button></div></form></Dialog>}
  </section>
}
