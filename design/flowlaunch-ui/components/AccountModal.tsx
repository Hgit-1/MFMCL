import { useState } from 'react';
import { Button } from './Button';
import { Input } from './Input';
import { Modal } from './Modal';
import { DefaultUserAvatar } from './DefaultUserAvatar';
import type { Account } from '../types';
import { useI18n } from '../utils/i18n';
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
const UploadIcon = () => <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2"><path d="M14 10v3a1 1 0 01-1 1H3a1 1 0 01-1-1v-3M8 2v9m4-5L8 2 4 6"/></svg>;
const PlusIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><path d="M10 5v10M5 10h10"/></svg>;
const ChevronRightIcon = () => <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2"><path d="M6 12l4-4-4-4"/></svg>;

type LoginMethod = 'microsoft' | 'offline' | 'thirdparty';

interface Props {
  isOpen: boolean;
  onClose: () => void;
  currentAccount?: Account;
  onLogin: (account: Account | undefined) => void;
  recentAccounts?: Account[];
  onUpdateAccount?: (account: Account) => void;
}

function SkinModel({ model }: { model: 'steve' | 'alex' }) {
  const { t } = useI18n();
  const modelName = model === 'steve' ? 'Steve' : 'Alex';
  return (
    <img
      className={styles.skinModel}
      src={`/demo-avatars/models/${model}-head.png`}
      alt={t('account.modelAlt', { values: { model: modelName } })}
      draggable={false}
    />
  );
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

export function AccountModal({ isOpen, onClose, currentAccount, onLogin, recentAccounts = [], onUpdateAccount }: Props) {
  const { t } = useI18n();
  const [method, setMethod] = useState<LoginMethod | null>(null);
  const [offlineName, setOfflineName] = useState('');
  const [serverUrl, setServerUrl] = useState('');
  const [editingAccount, setEditingAccount] = useState<Account | null>(null);
  const [editUsername, setEditUsername] = useState('');
  const [editSkinModel, setEditSkinModel] = useState<'steve' | 'alex'>('steve');
  const [hoveredAccountId, setHoveredAccountId] = useState<string | null>(null);
  const [showAddMenu, setShowAddMenu] = useState(false);

  const handleMicrosoftLogin = () => {
    const account: Account = {
      id: 'ms-demo',
      username: 'DemoPlayer',
      uuid: 'demo-uuid-ms',
      avatar: '/demo-avatars/notch-head.png',
      type: 'microsoft',
      hasMinecraft: true,
    };
    onLogin(account);
    onClose();
  };

  const handleOfflineLogin = () => {
    if (!offlineName.trim()) return;
    const account: Account = {
      id: 'offline-' + Date.now(),
      username: offlineName,
      uuid: 'offline-uuid-' + Date.now(),
      avatar: '/demo-avatars/jeb-head.png',
      type: 'offline',
      hasMinecraft: false,
    };
    onLogin(account);
    onClose();
  };

  const handleThirdPartyLogin = () => {
    if (!serverUrl.trim()) return;
    const account: Account = {
      id: 'authlib-demo',
      username: 'AuthlibPlayer',
      uuid: 'authlib-uuid',
      avatar: '/demo-avatars/dinnerbone-head.png',
      type: 'microsoft',
      hasMinecraft: true,
    };
    onLogin(account);
    onClose();
  };

  const handleEditAccount = (account: Account) => {
    setEditingAccount(account);
    setEditUsername(account.username);
    setEditSkinModel('steve');
  };

  const handleSaveEdit = () => {
    if (!editingAccount || !editUsername.trim()) return;
    const updatedAccount: Account = {
      ...editingAccount,
      username: editUsername,
    };
    if (onUpdateAccount) {
      onUpdateAccount(updatedAccount);
    }
    if (currentAccount?.id === editingAccount.id) {
      onLogin(updatedAccount);
    }
    setEditingAccount(null);
    setEditUsername('');
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
      <Modal isOpen={isOpen} onClose={() => setEditingAccount(null)} title={t('account.editTitle')} size="md">
        <div className={styles.editAccount}>
          <div className={styles.editPreview}>
            <PlayerAvatar username={editUsername || editingAccount.username} avatar={editingAccount.avatar} size={80} />
            <div className={styles.editInfo}>
              <h3>{editUsername || editingAccount.username}</h3>
              <p className={styles.accountType}>
                {editingAccount.type === 'microsoft' ? t('account.microsoft') : editingAccount.type === 'offline' ? t('account.offline') : t('account.thirdParty')}
              </p>
            </div>
          </div>

          <Input
            label={t('account.username')}
            value={editUsername}
            onChange={(e) => setEditUsername(e.target.value)}
            hint={editingAccount.type === 'microsoft' ? t('account.usernameHintMs') : t('account.usernameHintOffline')}
            disabled={editingAccount.type === 'microsoft'}
          />

          <div className={styles.skinUpload}>
            <label className={styles.uploadLabel}>
              <strong>{t('account.skinFile')}</strong>
              <span>{t('account.skinFileHint')}</span>
            </label>
            <div className={styles.uploadArea}>
              <UploadIcon />
              <span>{t('account.skinUpload')}</span>
              <input type="file" accept="image/png" style={{ display: 'none' }} />
            </div>
          </div>

          <div className={styles.modelSelect}>
            <label><strong>{t('account.skinModel')}</strong></label>
            <div className={styles.modelOptions}>
              <button
                type="button"
                className={`${styles.modelOption} ${editSkinModel === 'steve' ? styles.active : ''}`}
                onClick={() => setEditSkinModel('steve')}
                aria-pressed={editSkinModel === 'steve'}
              >
                <div className={styles.modelPreview}>
                  <SkinModel model="steve" />
                </div>
                <span>{t('account.skinSteve')}</span>
              </button>
              <button
                type="button"
                className={`${styles.modelOption} ${editSkinModel === 'alex' ? styles.active : ''}`}
                onClick={() => setEditSkinModel('alex')}
                aria-pressed={editSkinModel === 'alex'}
              >
                <div className={styles.modelPreview}>
                  <SkinModel model="alex" />
                </div>
                <span>{t('account.skinAlex')}</span>
              </button>
            </div>
          </div>

          {editingAccount.type === 'offline' && (
            <div className={styles.notice}>
              <AlertIcon />
              <p>{t('account.skinOfflineNotice')}</p>
            </div>
          )}

          <div className={styles.actions}>
            <Button onClick={handleSaveEdit} disabled={!editUsername.trim()}>
              {t('account.saveChanges')}
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

          {recentAccounts.length > 0 && (
            <aside className={styles.recentSidebar}>
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
              
              {/* 未登录状态的添加账户按钮 */}
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
          )}
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
            <Button onClick={handleMicrosoftLogin}>
              {t('account.msDemo')}
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

          <Input
            label={t('account.authServer')}
            placeholder="https://example.com/api/yggdrasil"
            value={serverUrl}
            onChange={(e) => setServerUrl(e.target.value)}
            hint={t('account.authServerHint')}
          />

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
            <Button onClick={handleThirdPartyLogin} disabled={!serverUrl.trim()}>
              {t('account.connectThirdParty')}
            </Button>
            <Button variant="ghost" onClick={resetView}>
              {t('common.back')}
            </Button>
          </div>
        </div>
      )}
    </Modal>
  );
}
