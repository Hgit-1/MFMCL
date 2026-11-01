import { useState } from 'react';
import { Button } from './Button';
import { Input } from './Input';
import { Modal } from './Modal';
import { DefaultUserAvatar } from './DefaultUserAvatar';
import type { Account } from '../types';
import { useI18n } from '../utils/i18n';
import { addOfflineAccount, errorText, microsoftLogin, removeAccount, skinLogin, skinSelectProfile } from '../utils/native';
import { toAccount } from '../utils/adapters';
import styles from './AccountModal.module.css';

// 简化图标
const MicrosoftIcon = () => (
  <svg width="21" height="21" viewBox="0 0 21 21" fill="none">
    <rect width="10" height="10" fill="#F25022"/>
    <rect x="11" width="10" height="10" fill="#7FBA00"/>
    <rect y="11" width="10" height="10" fill="#00A4EF"/>
    <rect x="11" y="11" width="10" height="10" fill="#FFB900"/>
  </svg>
);

const UserIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><circle cx="10" cy="7" r="4"/><path d="M3 18a7 7 0 0114 0"/></svg>;
const ServerIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><rect x="2" y="3" width="16" height="5" rx="2"/><rect x="2" y="12" width="16" height="5" rx="2"/><path d="M6 6h.01M6 15h.01"/></svg>;
const ShieldCheckIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><path d="M10 2L3 5v5c0 4.5 3 7 7 10 4-3 7-5.5 7-10V5l-7-3z"/><path d="M7 10l2 2 4-4"/></svg>;
const AlertIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><path d="M10 2l8 14H2L10 2z"/><path d="M10 8v3m0 3h.01"/></svg>;
const EditIcon = () => <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2"><path d="M11 2l3 3-9 9H2v-3l9-9z"/></svg>;
const LoginIcon = () => <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2"><path d="M10 2h4v12h-4M6 11l3-3-3-3M3 8h10"/></svg>;
const PlusIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><path d="M10 5v10M5 10h10"/></svg>;
const ChevronRightIcon = () => <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2"><path d="M6 12l4-4-4-4"/></svg>;

type LoginMethod = 'microsoft' | 'offline' | 'thirdparty';

interface Props {
  isOpen: boolean;
  onClose: () => void;
  currentAccount?: Account;
  onLogin: (account: Account | undefined) => void;
  recentAccounts?: Account[];
  /** 后端错误与操作结果通过它显示在应用外壳的提示条里。 */
  onNotice?: (message: string) => void;
  /** 账户列表发生变化（添加/移除）后通知外壳重新读取。 */
  onChanged?: () => void;
}

function PlayerAvatar({ username, avatar, size = 36 }: { username: string; avatar?: string; size?: number }) {
  const { t } = useI18n();
  return (
    <div className={styles.playerAvatarFrame} style={{ width: size, height: size }}>
      {avatar ? (
        <img className={styles.playerAvatar} src={avatar} alt={t('account.avatarAlt', { values: { name: username } })} />
      ) : (
        <DefaultUserAvatar className={styles.playerAvatarFallback} label={t('account.avatarPlaceholder', { values: { name: username } })} />
      )}
    </div>
  );
}

export function AccountModal({ isOpen, onClose, currentAccount, onLogin, recentAccounts = [], onNotice, onChanged }: Props) {
  const { t } = useI18n();
  const [method, setMethod] = useState<LoginMethod | null>(null);
  const [offlineName, setOfflineName] = useState('');
  const [serverUrl, setServerUrl] = useState('');
  const [editingAccount, setEditingAccount] = useState<Account | null>(null);
  /** 正在提交的后端操作；用于禁用按钮，避免重复提交。 */
  const [busy, setBusy] = useState(false);
  const [hoveredAccountId, setHoveredAccountId] = useState<string | null>(null);
  const [showAddMenu, setShowAddMenu] = useState(false);
  /** 正在等待后端的登录方式；用它禁用按钮并显示进度，避免重复点击开两个授权流程。 */
  const [pending, setPending] = useState<LoginMethod | null>(null);
  /** 皮肤站登录返回的待选角色列表；为空时表示还没到选择角色那一步。 */
  const [skinProfiles, setSkinProfiles] = useState<{ pendingId: string; profiles: { id: string; name: string }[] } | null>(null);
  const [skinTrusted, setSkinTrusted] = useState(false);
  const [skinPassword, setSkinPassword] = useState('');
  const [skinUsername, setSkinUsername] = useState('');

  const reportError = (error: unknown, fallback: string) => {
    onNotice?.(errorText(error, fallback));
  };

  /**
   * Microsoft 登录。
   *
   * 后端会打开系统浏览器完成 OAuth，并把账户写进数据库、标记为选中。这里不做
   * 任何本地账户拼装——凭据由后端保管，界面只拿到公开档案。
   */
  const handleMicrosoftLogin = () => {
    setPending('microsoft');
    void microsoftLogin()
      .then(profile => {
        onLogin(toAccount(profile));
        onClose();
      })
      .catch(error => reportError(error, 'Microsoft 登录失败。'))
      .finally(() => setPending(null));
  };

  /**
   * 离线档案。
   *
   * 名称规则由后端校验（3–16 位字母数字下划线）；这里先做同样的检查是为了
   * 立刻给出反馈，而不是等一次往返。
   */
  const handleOfflineLogin = () => {
    const name = offlineName.trim();
    if (!/^[A-Za-z0-9_]{3,16}$/.test(name)) {
      onNotice?.('离线档案名称需为 3–16 位英文字母、数字或下划线。');
      return;
    }
    setPending('offline');
    void addOfflineAccount(name)
      .then(profile => {
        onLogin(toAccount(profile));
        onClose();
      })
      .catch(error => reportError(error, '无法创建离线档案。'))
      .finally(() => setPending(null));
  };

  /**
   * 皮肤站（Yggdrasil）登录分两步。
   *
   * 第一步提交地址与凭据，后端返回该账户下的角色列表；第二步由用户选一个角色，
   * 才真正写入账户。之所以不在第一步自动选第一个角色，是因为同一个皮肤站账号
   * 可以有多个游戏角色，替用户猜会让他启动到错误的名字。
   */
  const handleThirdPartyLogin = () => {
    if (skinProfiles) {
      // 第二步：选择角色。`profileId` 为 null 时后端会拒绝，所以必须先选。
      const first = skinProfiles.profiles[0];
      if (!first) {
        onNotice?.('此账户没有可用的角色。');
        return;
      }
      setPending('thirdparty');
      void skinSelectProfile(skinProfiles.pendingId, first.id)
        .then(profile => {
          onLogin(toAccount(profile));
          onClose();
        })
        .catch(error => reportError(error, '无法选择角色。'))
        .finally(() => setPending(null));
      return;
    }

    if (!serverUrl.trim()) return;
    if (!skinTrusted) {
      onNotice?.('请先确认你信任此认证服务器。');
      return;
    }
    setPending('thirdparty');
    void skinLogin(serverUrl.trim(), skinUsername.trim(), skinPassword, skinTrusted)
      .then(result => {
        // 只有一个角色时不必让用户多点一次，直接完成第二步。
        if (result.profiles.length === 1) {
          return skinSelectProfile(result.pendingId, result.profiles[0].id).then(profile => {
            onLogin(toAccount(profile));
            onClose();
          });
        }
        setSkinProfiles(result);
        if (result.profiles.length === 0) onNotice?.('此账户没有可用的角色。');
      })
      .catch(error => reportError(error, '皮肤站登录失败。'))
      .finally(() => setPending(null));
  };

  const handleEditAccount = (account: Account) => {
    setEditingAccount(account);
  };

  /**
   * 移除账户。
   *
   * 后端会一并删掉系统凭据库里的令牌，所以先让用户确认——这个操作不可撤销，
   * 重新登录需要再走一次浏览器授权。界面不提供「重命名 / 换皮肤」：后端没有对应
   * 命令，名称与皮肤由账户服务本身决定，在这里改只会产生一个与服务器不一致的
   * 本地假象。
   */
  const handleRemoveAccount = async (account: Account) => {
    const extra = account.type === 'offline' ? '' : '\n\n系统凭据库中保存的登录令牌也会一并删除。';
    if (!window.confirm(`移除账户「${account.username}」？${extra}`)) return;
    setBusy(true);
    try {
      await removeAccount(account.id);
      onNotice?.(`账户「${account.username}」已移除。`);
      setEditingAccount(null);
      onChanged?.();
    } catch (error) {
      onNotice?.(errorText(error, '无法移除账户。'));
    } finally {
      setBusy(false);
    }
  };

  const resetView = () => {
    setMethod(null);
    setOfflineName('');
    setServerUrl('');
    setEditingAccount(null);
    setShowAddMenu(false);
  };

  if (editingAccount) {
    return (
      <Modal isOpen={isOpen} onClose={() => setEditingAccount(null)} title={t('account.management')} size="md">
        <div className={styles.editAccount}>
          <div className={styles.editPreview}>
            <PlayerAvatar username={editingAccount.username} avatar={editingAccount.avatar} size={80} />
            <div className={styles.editInfo}>
              <h3>{editingAccount.username}</h3>
              <p className={styles.accountType}>
                {editingAccount.type === 'microsoft' ? t('account.microsoft') : editingAccount.type === 'offline' ? t('account.offline') : t('account.thirdParty')}
              </p>
            </div>
          </div>

          <div className={styles.notice}>
            <AlertIcon />
            <p>
              {editingAccount.type === 'offline'
                ? '离线档案的名称与皮肤不能在这里修改；名称由离线档案本身决定，重新添加即可换一个名字。'
                : '账户名称与皮肤由账户服务提供，启动器不能修改；如需更换请移除后重新登录。'}
            </p>
          </div>

          <div className={styles.actions}>
            <Button variant="danger" disabled={busy} onClick={() => void handleRemoveAccount(editingAccount)}>
              {busy ? t('common.loading') : t('common.delete')}
            </Button>
            <Button variant="ghost" onClick={() => setEditingAccount(null)}>
              {t('common.cancel')}
            </Button>
          </div>
        </div>
      </Modal>
    );
  }


  if (currentAccount && !method) {
    return (
      <Modal isOpen={isOpen} onClose={onClose} title={t('account.management')} size="lg">
        <div className={styles.accountManagement}>
          <div className={styles.currentAccountSection}>
            <h3>{t('account.current')}</h3>
            <div className={styles.accountInfo}>
              <PlayerAvatar username={currentAccount.username} avatar={currentAccount.avatar} size={64} />
              <div className={styles.accountDetails}>
                <h4>{currentAccount.username}</h4>
                <p className={styles.accountType}>
                  {currentAccount.type === 'microsoft' ? t('account.microsoft') : currentAccount.type === 'offline' ? t('account.offline') : t('account.thirdParty')}
                </p>
                <div className={`${styles.ownershipBadge} ${currentAccount.hasMinecraft ? styles.verified : styles.unverified}`}>
                  {currentAccount.hasMinecraft ? (
                    <><ShieldCheckIcon /> {t('account.ownershipVerified')}</>
                  ) : (
                    <><AlertIcon /> {t('account.ownershipUnverified')}</>
                  )}
                </div>
              </div>
              <Button onClick={() => handleEditAccount(currentAccount)} size="sm" icon={<EditIcon />}>
                {t('common.edit')}
              </Button>
            </div>

            <div className={styles.notice}>
              <ShieldCheckIcon />
              <p>{t('account.switchSafe')}</p>
            </div>
          </div>

          <div className={styles.accountListSection}>
            {recentAccounts.length > 0 && <h3>{t('account.others')}</h3>}
            <div className={styles.accountGrid}>
              {recentAccounts.map((account) => (
                  <div
                    key={account.id}
                    className={styles.accountGridItem}
                    onMouseEnter={() => setHoveredAccountId(account.id)}
                    onMouseLeave={() => setHoveredAccountId(null)}
                  >
                    <div className={styles.gridItemContent}>
                      <PlayerAvatar username={account.username} avatar={account.avatar} size={48} />
                      <div className={styles.gridItemInfo}>
                        <span className={styles.gridItemName}>{account.username}</span>
                        <span className={styles.gridItemType}>
                          {account.type === 'microsoft' ? 'Microsoft' : account.type === 'offline' ? t('account.offlineShort') : t('account.thirdPartyShort')}
                        </span>
                      </div>
                      {account.hasMinecraft && (
                        <div className={styles.gridItemBadge}>
                          <ShieldCheckIcon />
                        </div>
                      )}
                    </div>
                    {hoveredAccountId === account.id && (
                      <div className={styles.gridItemActions}>
                        <button
                          className={styles.gridActionEdit}
                          onClick={(e) => {
                            e.stopPropagation();
                            handleEditAccount(account);
                          }}
                          title={t('common.edit')}
                        >
                          <EditIcon />
                        </button>
                        <button
                          className={styles.gridActionUse}
                          onClick={(e) => {
                            e.stopPropagation();
                            onLogin(account);
                          }}
                          title={t('account.switchTo')}
                        >
                          <LoginIcon />
                        </button>
                      </div>
                    )}
                  </div>
                ))}

                {/* 添加账户按钮 */}
                <div className={styles.accountGridItem}>
                  {!showAddMenu ? (
                    <button
                      type="button"
                      className={styles.addAccountButton}
                      onClick={() => setShowAddMenu(true)}
                      aria-expanded={showAddMenu}
                    >
                      <PlusIcon />
                      <span>{t('account.add')}</span>
                    </button>
                  ) : (
                    <div className={styles.addAccountMenu}>
                      <button onClick={() => { setShowAddMenu(false); setMethod('microsoft'); }}>
                        <MicrosoftIcon />
                        <span>Microsoft</span>
                        <ChevronRightIcon />
                      </button>
                      <button onClick={() => { setShowAddMenu(false); setMethod('offline'); }}>
                        <UserIcon />
                        <span>{t('account.offline')}</span>
                        <ChevronRightIcon />
                      </button>
                      <button onClick={() => { setShowAddMenu(false); setMethod('thirdparty'); }}>
                        <ServerIcon />
                        <span>{t('account.thirdPartyShort')}</span>
                        <ChevronRightIcon />
                      </button>
                    </div>
                  )}
                </div>
            </div>
          </div>

          <div className={styles.actions}>
            <Button onClick={onClose}>{t('common.close')}</Button>
          </div>
        </div>
      </Modal>
    );
  }

  return (
    <Modal isOpen={isOpen} onClose={() => { resetView(); onClose(); }} title={t('account.chooseMethod')} size="lg">
      {!method && (
        <div className={styles.layoutWithSidebar}>
          <div className={styles.welcomeSection}>
            <div className={styles.welcomeContent}>
              <div className={styles.welcomeIcon}>
                <div className={styles.logoIcon}>FL</div>
              </div>
              <h2>{t('account.welcome')}</h2>
              <p className={styles.welcomeDescription}>
                {t('account.welcomeHint')}
              </p>

              <div className={styles.infoCards}>
                <div className={styles.infoCard}>
                  <ShieldCheckIcon />
                  <div>
                    <h4>{t('account.secureLogin')}</h4>
                    <p>{t('account.secureLoginHint')}</p>
                  </div>
                </div>
                <div className={styles.infoCard}>
                  <UserIcon />
                  <div>
                    <h4>{t('account.multiAccount')}</h4>
                    <p>{t('account.multiAccountHint')}</p>
                  </div>
                </div>
                <div className={styles.infoCard}>
                  <ServerIcon />
                  <div>
                    <h4>{t('account.flexible')}</h4>
                    <p>{t('account.flexibleHint')}</p>
                  </div>
                </div>
              </div>

              <div className={styles.quickTips}>
                <h4>{t('account.quickTips')}</h4>
                <ul>
                  <li>{t('account.tipMicrosoft')}</li>
                  <li>{t('account.tipOffline')}</li>
                  <li>{t('account.tipThirdParty')}</li>
                </ul>
              </div>
            </div>

            <div className={styles.notice}>
              <ShieldCheckIcon />
              <p>{t('account.switchSafeLocal')}</p>
            </div>
          </div>

          {/*
            侧栏必须常驻：它是「添加账户」的唯一入口。此前整块被
            `recentAccounts.length > 0` 包住，零账户时连添加入口一起消失，
            新用户进来没有任何办法创建账户。现在只有账户列表按数量显示。
          */}
          <aside className={styles.recentSidebar}>
            {recentAccounts.length > 0 ? (
              <>
                <h4>{t('account.recent')}</h4>
                <div className={styles.recentList}>
                  {recentAccounts.map((account) => (
                    <div
                      key={account.id}
                      className={styles.recentAccountWrapper}
                      onMouseEnter={() => setHoveredAccountId(account.id)}
                      onMouseLeave={() => setHoveredAccountId(null)}
                    >
                      <div className={styles.recentAccount}>
                        <PlayerAvatar username={account.username} avatar={account.avatar} size={36} />
                        <div className={styles.recentInfo}>
                          <span className={styles.recentName}>{account.username}</span>
                          <span className={styles.recentType}>
                            {account.type === 'microsoft' ? 'Microsoft' : account.type === 'offline' ? t('account.offlineShort') : t('account.thirdPartyShort')}
                          </span>
                        </div>
                        {account.hasMinecraft && hoveredAccountId !== account.id && (
                          <div className={styles.recentBadge}>
                            <ShieldCheckIcon />
                          </div>
                        )}
                        {hoveredAccountId === account.id && (
                          <div className={styles.recentActions}>
                            <button
                              className={styles.recentActionEdit}
                              onClick={(e) => {
                                e.stopPropagation();
                                handleEditAccount(account);
                              }}
                            >
                              <EditIcon />
                              <span>{t('account.modify')}</span>
                            </button>
                            <button
                              className={styles.recentActionUse}
                              onClick={(e) => {
                                e.stopPropagation();
                                onLogin(account);
                                onClose();
                              }}
                            >
                              <LoginIcon />
                              <span>{t('account.use')}</span>
                            </button>
                          </div>
                        )}
                      </div>
                    </div>
                  ))}
                </div>
                <p className={styles.recentHint}>{t('account.hoverHint')}</p>
              </>
            ) : (
              <p className={styles.recentEmpty}>{t('account.noAccountsYet')}</p>
            )}

            {/* 添加账户入口：无论是否已有账户都显示，否则新用户没有可点的地方。 */}
            {!showAddMenu ? (
              <button
                className={styles.addAccountBtn}
                onClick={() => setShowAddMenu(true)}
              >
                <PlusIcon />
                <span>{t('account.add')}</span>
              </button>
            ) : (
              <div className={styles.addAccountMenu}>
                <button onClick={() => { setShowAddMenu(false); setMethod('microsoft'); }}>
                  <MicrosoftIcon />
                  <span>Microsoft</span>
                  <ChevronRightIcon />
                </button>
                <button onClick={() => { setShowAddMenu(false); setMethod('offline'); }}>
                  <UserIcon />
                  <span>{t('account.offline')}</span>
                  <ChevronRightIcon />
                </button>
                <button onClick={() => { setShowAddMenu(false); setMethod('thirdparty'); }}>
                  <ServerIcon />
                  <span>{t('account.thirdPartyShort')}</span>
                  <ChevronRightIcon />
                </button>
              </div>
            )}
          </aside>
        </div>
      )}

      {method === 'microsoft' && (
        <div className={styles.methodDetail}>
          <div className={styles.detailHeader}>
            <div className={styles.methodIcon}>
              <MicrosoftIcon />
            </div>
            <div>
              <h3>{t('account.microsoftLogin')}</h3>
              <p>{t('account.microsoftLoginHint')}</p>
            </div>
          </div>

          <div className={styles.infoBox}>
            <h4>{t('account.loginFlow')}</h4>
            <ol>
              <li>{t('account.step1')}</li>
              <li>{t('account.step2')}</li>
              <li>{t('account.step3')}</li>
              <li>{t('account.step4')}</li>
            </ol>
          </div>

          <div className={styles.notice}>
            <ShieldCheckIcon />
            <p>{t('account.msPrivacy')}</p>
          </div>

          <div className={styles.actions}>
            <Button onClick={handleMicrosoftLogin} disabled={pending !== null}>
              {pending === 'microsoft' ? '正在打开浏览器…' : t('account.microsoftLogin')}
            </Button>
            <Button variant="ghost" onClick={resetView}>
              {t('common.back')}
            </Button>
          </div>
        </div>
      )}

      {method === 'offline' && (
        <div className={styles.methodDetail}>
          <div className={styles.detailHeader}>
            <div className={styles.methodIcon}>
              <UserIcon />
            </div>
            <div>
              <h3>{t('account.offline')}</h3>
              <p>{t('account.offlineHint')}</p>
            </div>
          </div>

          <Input
            label={t('account.username')}
            placeholder={t('account.usernamePlaceholder')}
            value={offlineName}
            onChange={(e) => setOfflineName(e.target.value)}
            hint={t('account.offlineUsernameHint')}
          />

          <div className={`${styles.notice} ${styles.warning}`}>
            <AlertIcon />
            <div>
              <h4>{t('account.offlineLimits')}</h4>
              <ul>
                <li>{t('account.offlineLimit1')}</li>
                <li>{t('account.offlineLimit2')}</li>
                <li>{t('account.offlineLimit3')}</li>
                <li>{t('account.offlineLimit4')}</li>
              </ul>
            </div>
          </div>

          <div className={styles.actions}>
            <Button onClick={handleOfflineLogin} disabled={!offlineName.trim()}>
              {t('account.useOffline')}
            </Button>
            <Button variant="ghost" onClick={resetView}>
              {t('common.back')}
            </Button>
          </div>
        </div>
      )}

      {method === 'thirdparty' && (
        <div className={styles.methodDetail}>
          <div className={styles.detailHeader}>
            <div className={styles.methodIcon}>
              <ServerIcon />
            </div>
            <div>
              <h3>{t('account.thirdPartyServer')}</h3>
              <p>{t('account.thirdPartyHint')}</p>
            </div>
          </div>

          {/*
            角色选择阶段：登录已经成功，只是该账户下有多个角色。
            这时不再显示地址与凭据输入框——凭据已经交给后端，重新显示只会让人
            以为要重填一遍。
          */}
          {skinProfiles ? (
            <>
              <div className={styles.infoBox}>
                <h4>选择角色</h4>
                <p>此账户下有 {skinProfiles.profiles.length} 个角色。启动时使用的就是这里选中的名字。</p>
              </div>
              <div className={styles.actions}>
                {skinProfiles.profiles.map(profile => (
                  <Button
                    key={profile.id}
                    disabled={pending !== null}
                    onClick={() => {
                      setPending('thirdparty');
                      void skinSelectProfile(skinProfiles.pendingId, profile.id)
                        .then(account => {
                          onLogin(toAccount(account));
                          onClose();
                        })
                        .catch(error => reportError(error, '无法选择角色。'))
                        .finally(() => setPending(null));
                    }}
                  >
                    {profile.name}
                  </Button>
                ))}
              </div>
              <div className={styles.actions}>
                <Button variant="ghost" onClick={resetView}>{t('common.back')}</Button>
              </div>
            </>
          ) : (
            <>
              <Input
                label={t('account.authServer')}
                placeholder="https://example.com/api/yggdrasil"
                value={serverUrl}
                onChange={(e) => setServerUrl(e.target.value)}
                hint={t('account.authServerHint')}
              />
              <Input
                label={t('account.username')}
                value={skinUsername}
                autoComplete="username"
                onChange={(e) => setSkinUsername(e.target.value)}
              />
              <Input
                label="密码"
                type="password"
                value={skinPassword}
                autoComplete="current-password"
                onChange={(e) => setSkinPassword(e.target.value)}
                hint="凭据只发送到你确认的认证地址，不会写入日志或本地明文存储。"
              />

              {/*
                信任确认。后端在没有这个确认时直接拒绝登录，因为密码会被发往
                用户填写的地址——必须让人先明确看过这个地址。
              */}
              <label className={styles.trustRow}>
                <input
                  type="checkbox"
                  checked={skinTrusted}
                  onChange={e => setSkinTrusted(e.target.checked)}
                />
                <span>我确认信任 <code>{serverUrl || '此地址'}</code>，并了解密码会被发送到该服务器。</span>
              </label>

              <div className={styles.infoBox}>
                <h4>{t('account.whatIsThirdParty')}</h4>
                <p>{t('account.thirdPartyExplain')}</p>
                <ul>
                  <li>{t('account.thirdPartyNeed1')}</li>
                  <li>{t('account.thirdPartyNeed2')}</li>
                </ul>
              </div>

              <div className={`${styles.notice} ${styles.warning}`}>
                <AlertIcon />
                <div>
                  <h4>{t('account.securityTip')}</h4>
                  <p>{t('account.securityWarning')}</p>
                </div>
              </div>

              <div className={styles.actions}>
                <Button
                  onClick={handleThirdPartyLogin}
                  disabled={!serverUrl.trim() || !skinUsername.trim() || !skinPassword || !skinTrusted || pending !== null}
                >
                  {pending === 'thirdparty' ? '正在登录…' : t('account.connectThirdParty')}
                </Button>
                <Button variant="ghost" onClick={resetView}>
                  {t('common.back')}
                </Button>
              </div>
            </>
          )}
        </div>
      )}
    </Modal>
  );
}
