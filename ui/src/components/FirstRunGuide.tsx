import { CircleCheck, CircleDashed, Gamepad2, Cpu, Layers3, Play, ArrowRight } from 'lucide-react'

type Step = { key: string; title: string; detail: string; done: boolean; action?: { label: string; run: () => void } }

// Plan stage 6: a first-run guided path so a new user can reach a running game
// without understanding JVM terminology. Steps reflect real backend state only.
export function FirstRunGuide({ hasAccount, javaReady, instanceCount, anyReady, onAccount, onSettings, onCreate }: { hasAccount: boolean; javaReady: boolean; instanceCount: number; anyReady: boolean; onAccount: () => void; onSettings: () => void; onCreate: () => void }) {
  const steps: Step[] = [
    { key: 'account', title: '添加游戏账户', detail: hasAccount ? '账户已就绪，可在左下角切换。' : 'Microsoft 正版、皮肤站或离线档案（离线每次启动都会确认）。', done: hasAccount, action: hasAccount ? undefined : { label: '添加账户', run: onAccount } },
    { key: 'java', title: '准备 Java 运行环境', detail: javaReady ? '已检测到可用的 Java。' : '未检测到 Java 时，可在“设置 → 高级设置”手动指定路径。', done: javaReady, action: javaReady ? undefined : { label: '打开设置', run: onSettings } },
    { key: 'instance', title: '创建第一个实例', detail: '每个实例拥有独立的游戏目录、存档与模组，互不干扰。', done: instanceCount > 0, action: instanceCount > 0 ? undefined : { label: '创建实例', run: onCreate } },
    { key: 'launch', title: '安装并启动游戏', detail: '安装会校验每个文件的哈希；完成后点击“启动游戏”即可。', done: anyReady },
  ]
  const doneCount = steps.filter(step => step.done).length
  return <section className="guide-card panel" aria-label="新手引导">
    <div className="panel-heading"><div><h2>开始指南</h2><p>{doneCount === steps.length ? '所有步骤都已完成，祝你玩得开心。' : `已完成 ${doneCount}/${steps.length} 步 · 按顺序操作即可开始游戏`}</p></div></div>
    <ol className="guide-steps">
      {steps.map((step, index) => <li key={step.key} className={step.done ? 'done' : ''}>
        <span className="guide-step-icon">{step.done ? <CircleCheck size={18} /> : <CircleDashed size={18} />}</span>
        <span className="guide-step-copy"><strong><small>{index + 1}</small>{step.title}</strong><span>{step.detail}</span></span>
        {step.action && <button className="button-secondary small" onClick={step.action.run}>{step.action.label}<ArrowRight size={13} /></button>}
      </li>)}
    </ol>
    <p className="guide-note"><Gamepad2 size={13} /> 离线档案不验证正版授权，仅适合本地或离线模式服务器；<Cpu size={13} /> Java 缺失时大多数版本无法启动；<Layers3 size={13} /> 建议每个玩法一个实例；<Play size={13} /> 启动后在概览页可查看单行日志与关闭进程。</p>
  </section>
}
