import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { ArrowDownToLine, CircleAlert, CircleCheck, RefreshCw, ShieldCheck } from 'lucide-react'
import { checkLauncherUpdate, downloadLauncherUpdate, isDesktopShell, type LauncherUpdateStatus } from '../api'
import './update-card.css'

type Phase = 'idle' | 'checking' | 'downloading' | 'done'

// Interaction follows PCL2's public settings/about wording (policy choice +
// manual check + changelog); the implementation is original and deliberately
// downloads a verified installer instead of self-replacing a running binary.
export function UpdateCard({ policy, onPolicyChange, onNotice }: { policy: LauncherSettingsPolicy; onPolicyChange: (policy: LauncherSettingsPolicy) => void; onNotice: (message: string) => void }) {
  const [phase, setPhase] = useState<Phase>('idle')
  const [status, setStatus] = useState<LauncherUpdateStatus | null>(null)
  const [error, setError] = useState('')
  const [version, setVersion] = useState('0.1.0')

  useEffect(() => {
    if (!isDesktopShell()) return
    void invoke<{ appVersion: string }>('get_system_info').then(info => setVersion(info.appVersion)).catch(() => undefined)
  }, [])

  const runCheck = async (announceResult: boolean) => {
    setPhase('checking')
    setError('')
    try {
      const result = await checkLauncherUpdate()
      setStatus(result)
      setVersion(result.current || version)
      if (announceResult) onNotice(result.available ? `发现新版本 ${result.latest}。` : '当前已是最新版本。')
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason)
      if (announceResult) { setError(message); onNotice(message) }
    } finally {
      if (phase !== 'downloading') setPhase('idle')
    }
  }

  useEffect(() => {
    // Startup check runs quietly when the user chose reminders, mirroring PCL2's
    // background update check; failures stay silent until a manual check.
    if (isDesktopShell() && policy === 'notify') void runCheck(false)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const download = async () => {
    if (!status?.available) return
    const directory = await open({ directory: true, multiple: false, title: '选择更新安装包保存文件夹' })
    if (typeof directory !== 'string') return
    setPhase('downloading')
    try {
      const saved = await downloadLauncherUpdate(status, directory)
      setPhase('done')
      onNotice(`更新安装包已校验并保存到 ${saved}。请运行安装包完成更新后重启 MFMCL。`)
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
      setPhase('idle')
    }
  }

  return <section className="settings-card update-card">
    <div className="settings-card-heading"><div className="setting-heading-icon icon-download"><RefreshCw size={17} /></div><div><h2>启动器更新</h2><p>当前版本 {version} · 更新前会校验安装包完整性。</p></div></div>
    <div className="setting-field"><div className="field-copy"><strong>更新提醒</strong><span>启动时在后台检查新版本并提醒。</span></div>
      <div className="source-options">
        <UpdateChoice active={policy === 'notify'} title="自动提醒" detail="启动时检查" onClick={() => onPolicyChange('notify')} />
        <UpdateChoice active={policy === 'off'} title="关闭" detail="不检查更新" onClick={() => onPolicyChange('off')} />
      </div>
    </div>
    <div className="setting-divider" />
    {status?.available ? <div className="update-available" role="status">
      <div className="update-available-copy"><CircleCheck size={16} /><div><strong>新版本 {status.latest} 可用</strong>{status.notes && <p>{status.notes}</p>}</div></div>
      <button className="button-primary" disabled={phase === 'downloading'} onClick={() => void download()}><ArrowDownToLine size={15} />{phase === 'downloading' ? '正在下载并校验…' : '下载安装包'}</button>
      {phase === 'done' && <small>安装包已保存；运行安装包并重启 MFMCL 后生效。MFMCL 不会自动替换正在运行的程序。</small>}
    </div> : status && <p className="update-status-line" role="status"><CircleCheck size={14} /> 已是最新版本（{status.latest ?? version}）。</p>}
    {error && <p className="management-error update-error" role="alert"><CircleAlert size={15} />{error}</p>}
    <div className="update-actions">
      <button className="button-secondary" disabled={phase === 'checking' || phase === 'downloading'} onClick={() => void runCheck(true)}><RefreshCw size={15} className={phase === 'checking' ? 'update-spinning' : undefined} />{phase === 'checking' ? '正在检查…' : '立即检查更新'}</button>
      {!isDesktopShell() && <small className="update-preview-note"><ShieldCheck size={13} />更新检查需要桌面应用后端。</small>}
    </div>
  </section>
}

type LauncherSettingsPolicy = 'notify' | 'off'
function UpdateChoice({ active, title, detail, onClick }: { active: boolean; title: string; detail: string; onClick: () => void }) {
  return <button type="button" className={`source-choice ${active ? 'selected' : ''}`} onClick={onClick}><span className="source-choice-copy"><strong>{title}</strong><small>{detail}</small></span></button>
}
