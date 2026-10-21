import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { Pause, Play, X, RotateCcw } from 'lucide-react'
import { isDesktopShell } from '../api'
import './installation-tasks.css'

type State = 'running' | 'paused' | 'completed' | 'cancelled' | 'failed' | 'interrupted'
interface Task { id: string; instanceId: string; label: string; source: string | null; state: State; completedBytes: number; totalBytes: number | null; updatedAt: string }
const labels: Record<State, string> = { running: '安装进行中', paused: '已暂停', completed: '安装完成', cancelled: '已取消', failed: '安装失败', interrupted: '上次安装被中断' }
export function InstallationTasks({ onNotice, onRetry, retryableInstances }: { onNotice: (message: string) => void; onRetry: (instanceId: string) => void; retryableInstances: string[] }) {
  const [tasks, setTasks] = useState<Task[]>([])
  const [busy, setBusy] = useState<string | null>(null)
  useEffect(() => {
    if (!isDesktopShell()) return
    let disposed = false
    let unlisten: (() => void) | undefined
    const changed = new Set<string>()
    // Subscribe before loading history so a slow database response cannot overwrite a newer event.
    void listen<{ instanceId: string; task: Task }>('installation-task', ({ payload }) => {
      if (disposed) return
      changed.add(payload.task.id)
      setTasks(current => [payload.task, ...current.filter(item => item.id !== payload.task.id)].slice(0, 100))
    }).then(async dispose => {
      if (disposed) { dispose(); return }
      unlisten = dispose
      const history = await invoke<Task[]>('list_download_tasks')
      if (!disposed && Array.isArray(history)) setTasks(current => [...current, ...history.filter(item => !changed.has(item.id))].slice(0, 100))
    }).catch(() => { if (!disposed) onNotice('无法读取下载任务记录。') })
    return () => { disposed = true; unlisten?.() }
  }, [onNotice])
  const control = async (task: Task, action: 'pause' | 'resume' | 'cancel') => {
    if (action === 'cancel' && !window.confirm('取消本次安装？已验证的文件会保留，之后可以重试。')) return
    setBusy(task.id)
    try { await invoke('control_download', { instanceId: task.instanceId, action }) }
    catch (error) { onNotice(error instanceof Error ? error.message : String(error)) }
    finally { setBusy(null) }
  }
  if (!tasks.length) return null
  return <section className="installation-history panel" aria-label="安装任务与历史记录">
    <div className="panel-heading"><div><h2>安装任务与历史</h2><p>暂停在当前下载或安装步骤的安全检查点生效。取消后可使用已校验文件重试。</p></div></div>
    {tasks.map(task => {
      const active = task.state === 'running' || task.state === 'paused'
      return <article className="installation-history-row" key={task.id}>
        <div className="installation-history-copy"><strong>{task.label}</strong><span>{labels[task.state]}{active ? ' · 等待所有步骤完成后才可启动' : ''}</span>{task.source && <small title={task.source}>来源：{task.source}</small>}{active && <small>当前文件：{(task.completedBytes / 1048576).toFixed(1)} MB{task.totalBytes != null ? ` / ${(task.totalBytes / 1048576).toFixed(1)} MB` : ' · 大小未知'}</small>}</div>
        <div className="installation-history-actions">{active ? <><button className="button-secondary small" disabled={busy === task.id} onClick={() => void control(task, task.state === 'paused' ? 'resume' : 'pause')}>{task.state === 'paused' ? <Play size={15} /> : <Pause size={15} />}{task.state === 'paused' ? '继续' : '暂停'}</button><button className="button-secondary small" disabled={busy === task.id} onClick={() => void control(task, 'cancel')}><X size={15} />取消</button></> : task.state !== 'completed' && task.instanceId && retryableInstances.includes(task.instanceId) && !tasks.some(other => other.instanceId === task.instanceId && (other.state === 'running' || other.state === 'paused')) && <button className="button-secondary small" onClick={() => onRetry(task.instanceId)}><RotateCcw size={15} />重试安装</button>}</div>
      </article>
    })}
  </section>
}
