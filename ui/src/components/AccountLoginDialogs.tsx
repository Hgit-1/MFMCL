import { useState, type FormEvent } from 'react'
import { ArrowRight, Check, LockKeyhole, ShieldCheck, Sparkles } from 'lucide-react'
import { Dialog } from './Dialog'
import { useModalClose } from './modal-close'
import { microsoftLogin, skinLogin, skinSelectProfile } from '../api'
import type { AccountRecord, SkinLoginResult } from '../types'

function loginError(reason: unknown, fallback: string): string {
  return reason instanceof Error ? reason.message : typeof reason === 'string' ? reason : fallback
}

type AccountLoginDialogsProps = {
  onClose: () => void
  onAuthenticated: (account: AccountRecord) => void
}

export function MicrosoftLoginDialog({ onClose, onAuthenticated }: AccountLoginDialogsProps) {
  const close = useModalClose(onClose)
  const [clientId, setClientId] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [waitingForBrowser, setWaitingForBrowser] = useState(false)

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setError('')
    setBusy(true)
    setWaitingForBrowser(true)
    try {
      const account = await microsoftLogin(clientId)
      onAuthenticated(account)
    } catch (reason) {
      setError(loginError(reason, 'Microsoft 登录失败，请重试。'))
    } finally {
      setBusy(false)
      setWaitingForBrowser(false)
    }
  }

  return <Dialog title="连接 Microsoft 账户" onClose={onClose} dismissible={!busy}>
    <form onSubmit={submit} className="dialog-form account-login-form">
      <div className="warning-box neutral"><ShieldCheck size={16} /><span>登录会在系统浏览器中完成。MFMCL 最多等待 180 秒，账户凭据由桌面端保存，不会传入网页界面。</span></div>
      <label className="form-label">公共客户端 ID（可留空使用构建配置）<input autoFocus disabled={busy} value={clientId} onChange={(event) => setClientId(event.target.value)} autoComplete="off" spellCheck={false} placeholder="留空使用此构建已配置的客户端 ID" /></label>
      <p className="input-hint account-login-hint">开发构建可填写你自己注册并获准调用 Minecraft 服务的公共客户端 ID。请勿使用他人的应用 ID。</p>
      {waitingForBrowser && <p className="account-login-progress" role="status">正在打开系统浏览器并等待授权回调…</p>}
      {error && <p className="management-error account-login-error" role="alert">{error}</p>}
      <div className="dialog-actions"><button type="button" className="button-secondary" onClick={close} disabled={busy}>取消</button><button type="submit" className="button-primary" disabled={busy}>{busy ? '等待授权…' : '在浏览器中登录'}<ArrowRight size={14} /></button></div>
    </form>
  </Dialog>
}

export function SkinLoginDialog({ onClose, onAuthenticated }: AccountLoginDialogsProps) {
  const close = useModalClose(onClose)
  const [server, setServer] = useState('')
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [trusted, setTrusted] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [login, setLogin] = useState<SkinLoginResult | null>(null)
  const [profileId, setProfileId] = useState('')

  const submitLogin = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const submittedPassword = password
    setPassword('')
    setError('')
    setBusy(true)
    try {
      const result = await skinLogin(server, username, submittedPassword, trusted)
      setLogin(result)
      setProfileId(result.profiles.length === 1 ? result.profiles[0].id : '')
      if (!result.profiles.length) setError('此账户没有可用角色，请检查皮肤站账户设置。')
    } catch (reason) {
      setError(loginError(reason, '皮肤站登录失败，请检查地址和账户信息。'))
    } finally {
      setBusy(false)
    }
  }

  const submitProfile = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!login) return
    setError('')
    setBusy(true)
    try {
      const account = await skinSelectProfile(login.pendingId, profileId || null)
      onAuthenticated(account)
    } catch (reason) {
      setError(`${loginError(reason, '无法保存所选角色。')} 请返回重新登录后再选择角色。`)
    } finally {
      setBusy(false)
    }
  }

  return <Dialog title={login ? '选择游戏角色' : '连接皮肤站'} onClose={onClose} dismissible={!busy}>
    {login ? <form onSubmit={submitProfile} className="dialog-form account-login-form">
      <div className="warning-box neutral"><Sparkles size={16} /><span>登录已通过。选择要用于游戏的角色；认证凭据仍由桌面端安全保存。</span></div>
      {login.profiles.length ? <label className="form-label">游戏角色<select autoFocus required disabled={busy} value={profileId} onChange={(event) => setProfileId(event.target.value)}><option value="">选择一个角色</option>{login.profiles.map((profile) => <option key={profile.id} value={profile.id}>{profile.name}</option>)}</select></label> : <p className="management-error" role="alert">账户未提供可选角色。</p>}
      {error && <p className="management-error account-login-error" role="alert">{error}</p>}
      <div className="dialog-actions"><button type="button" className="button-secondary" onClick={() => { setLogin(null); setError('') }} disabled={busy}>返回登录</button><button type="submit" className="button-primary" disabled={busy || !login.profiles.length || !profileId}>{busy ? '正在保存…' : '确认角色'}<Check size={14} /></button></div>
    </form> : <form onSubmit={submitLogin} className="dialog-form account-login-form">
      <div className="warning-box neutral"><ShieldCheck size={16} /><span>请核对下面显示的完整 HTTPS 根地址，只向你明确授权的皮肤站提交凭据。登录密码在提交时立即清空，不会保存到界面。</span></div>
      <label className="form-label">HTTPS 认证根地址<input autoFocus disabled={busy} value={server} onChange={(event) => { setServer(event.target.value); setTrusted(false) }} type="url" inputMode="url" autoComplete="url" placeholder="https://auth.example.com/" required /><span className="input-hint">必须是 HTTPS 根地址，不含账号、查询参数或片段。</span></label>
      <div className="skin-trust-summary" aria-live="polite"><span>将连接到</span><strong>{server.trim() || '请先输入 HTTPS 认证根地址'}</strong></div>
      <label className="form-label">皮肤站用户名<input disabled={busy} value={username} onChange={(event) => setUsername(event.target.value)} autoComplete="username" required /></label>
      <label className="form-label">密码<input disabled={busy} value={password} onChange={(event) => setPassword(event.target.value)} type="password" autoComplete="current-password" required /></label>
      <label className="skin-trust-check"><input type="checkbox" disabled={busy} checked={trusted} onChange={(event) => setTrusted(event.target.checked)} required /><span>我已核对上述 HTTPS 地址，并信任此皮肤站接收我的登录凭据。</span></label>
      {error && <p className="management-error account-login-error" role="alert">{error}</p>}
      <div className="dialog-actions"><button type="button" className="button-secondary" onClick={close} disabled={busy}>取消</button><button type="submit" className="button-primary" disabled={busy || !trusted}>{busy ? '正在安全连接…' : '登录并选择角色'}<LockKeyhole size={14} /></button></div>
    </form>}
  </Dialog>
}
