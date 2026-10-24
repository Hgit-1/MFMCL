import React, { useState } from 'react';
import { Button } from './Button';
import { Input } from './Input';
import { Modal } from './Modal';
import type { Account } from '../types';
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
  const modelName = model === 'steve' ? 'Steve' : 'Alex';
  return (
    <img
      className={styles.skinModel}
      src={`/demo-avatars/models/${model}-head.png`}
      alt={`${modelName} 头颅预览`}
      draggable={false}
    />
  );
}

function PlayerAvatar({ username, avatar, size = 36 }: { username: string; avatar?: string; size?: number }) {
  return (
    <div className={styles.playerAvatarFrame} style={{ width: size, height: size }}>
      {avatar ? (
        <img className={styles.playerAvatar} src={avatar} alt={`${username} 的 Minecraft 头颅`} />
      ) : (
        <span className={styles.playerAvatarFallback} aria-label={`${username} 的头像占位符`}>{username.slice(0, 1).toUpperCase()}</span>
      )}
    </div>
  );
}

export function AccountModal({ isOpen, onClose, currentAccount, onLogin, recentAccounts = [], onUpdateAccount }: Props) {
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
      <Modal isOpen={isOpen} onClose={() => setEditingAccount(null)} title="编辑账户" size="md">
        <div className={styles.editAccount}>
          <div className={styles.editPreview}>
            <PlayerAvatar username={editUsername || editingAccount.username} avatar={editingAccount.avatar} size={80} />
            <div className={styles.editInfo}>
              <h3>{editUsername || editingAccount.username}</h3>
              <p className={styles.accountType}>
                {editingAccount.type === 'microsoft' ? 'Microsoft 账户' : editingAccount.type === 'offline' ? '离线模式' : '第三方认证'}
              </p>
            </div>
          </div>

          <Input
            label="用户名"
            value={editUsername}
            onChange={(e) => setEditUsername(e.target.value)}
            hint={editingAccount.type === 'microsoft' ? 'Microsoft 账户用户名由微软管理' : '3-16个字符，字母、数字和下划线'}
            disabled={editingAccount.type === 'microsoft'}
          />

          <div className={styles.skinUpload}>
            <label className={styles.uploadLabel}>
              <strong>皮肤文件</strong>
              <span>PNG 格式，64×64 或 64×32 像素</span>
            </label>
            <div className={styles.uploadArea}>
              <UploadIcon />
              <span>点击上传皮肤文件</span>
              <input type="file" accept="image/png" style={{ display: 'none' }} />
            </div>
          </div>

          <div className={styles.modelSelect}>
            <label><strong>皮肤模型</strong></label>
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
                <span>Steve（粗臂）</span>
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
                <span>Alex（细臂）</span>
              </button>
            </div>
          </div>

          {editingAccount.type === 'offline' && (
            <div className={styles.notice}>
              <AlertIcon />
              <p>离线模式的皮肤仅在本地生效。多人游戏中的皮肤显示由服务器决定。</p>
            </div>
          )}

          <div className={styles.actions}>
            <Button onClick={handleSaveEdit} disabled={!editUsername.trim()}>
              保存更改
            </Button>
            <Button variant="ghost" onClick={() => setEditingAccount(null)}>
              取消
            </Button>
          </div>
        </div>
      </Modal>
    );
  }

  if (currentAccount && !method) {
    return (
      <Modal isOpen={isOpen} onClose={onClose} title="账户管理" size="lg">
        <div className={styles.accountManagement}>
          <div className={styles.currentAccountSection}>
            <h3>当前账户</h3>
            <div className={styles.accountInfo}>
              <PlayerAvatar username={currentAccount.username} avatar={currentAccount.avatar} size={64} />
              <div className={styles.accountDetails}>
                <h4>{currentAccount.username}</h4>
                <p className={styles.accountType}>
                  {currentAccount.type === 'microsoft' ? 'Microsoft 账户' : currentAccount.type === 'offline' ? '离线模式' : '第三方认证'}
                </p>
                <div className={`${styles.ownershipBadge} ${currentAccount.hasMinecraft ? styles.verified : styles.unverified}`}>
                  {currentAccount.hasMinecraft ? (
                    <><ShieldCheckIcon /> Java 版所有权已确认</>
                  ) : (
                    <><AlertIcon /> 未验证所有权</>
                  )}
                </div>
              </div>
              <Button onClick={() => handleEditAccount(currentAccount)} size="sm" icon={<EditIcon />}>
                编辑
              </Button>
            </div>

            <div className={styles.notice}>
              <ShieldCheckIcon />
              <p>切换账户不会影响已安装的游戏、模组或存档。</p>
            </div>
          </div>

          <div className={styles.accountListSection}>
            {recentAccounts.length > 0 && <h3>其他账户</h3>}
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
                          {account.type === 'microsoft' ? 'Microsoft' : account.type === 'offline' ? '离线' : '第三方'}
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
                          title="编辑"
                        >
                          <EditIcon />
                        </button>
                        <button
                          className={styles.gridActionUse}
                          onClick={(e) => {
                            e.stopPropagation();
                            onLogin(account);
                          }}
                          title="切换到此账户"
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
                      <span>添加账户</span>
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
                        <span>离线模式</span>
                        <ChevronRightIcon />
                      </button>
                      <button onClick={() => { setShowAddMenu(false); setMethod('thirdparty'); }}>
                        <ServerIcon />
                        <span>第三方</span>
                        <ChevronRightIcon />
                      </button>
                    </div>
                  )}
                </div>
            </div>
          </div>

          <div className={styles.actions}>
            <Button onClick={onClose}>关闭</Button>
          </div>
        </div>
      </Modal>
    );
  }

  return (
    <Modal isOpen={isOpen} onClose={() => { resetView(); onClose(); }} title="选择登录方式" size="lg">
      {!method && (
        <div className={styles.layoutWithSidebar}>
          <div className={styles.welcomeSection}>
            <div className={styles.welcomeContent}>
              <div className={styles.welcomeIcon}>
                <div className={styles.logoIcon}>FL</div>
              </div>
              <h2>欢迎使用 FlowLaunch</h2>
              <p className={styles.welcomeDescription}>
                选择右侧的账户登录，或点击"添加账户"按钮添加新账户。
                <br />
                支持微软账户、离线模式和第三方认证服务。
              </p>

              <div className={styles.infoCards}>
                <div className={styles.infoCard}>
                  <ShieldCheckIcon />
                  <div>
                    <h4>安全登录</h4>
                    <p>微软账户通过系统浏览器安全授权，不会保存明文密码</p>
                  </div>
                </div>
                <div className={styles.infoCard}>
                  <UserIcon />
                  <div>
                    <h4>多账户管理</h4>
                    <p>支持同时管理多个账户，快速切换无需重复登录</p>
                  </div>
                </div>
                <div className={styles.infoCard}>
                  <ServerIcon />
                  <div>
                    <h4>灵活接入</h4>
                    <p>支持离线模式和自定义认证服务器，适应不同使用场景</p>
                  </div>
                </div>
              </div>

              <div className={styles.quickTips}>
                <h4>快速提示</h4>
                <ul>
                  <li>推荐使用 Microsoft 账户获得完整功能支持</li>
                  <li>离线模式仅适用于单人游戏或支持离线的服务器</li>
                  <li>第三方认证需要从服务器管理员获取 API 地址</li>
                </ul>
              </div>
            </div>

            <div className={styles.notice}>
              <ShieldCheckIcon />
              <p>切换账户不会影响已安装的游戏、模组或存档。所有账户数据仅存储在本地。</p>
            </div>
          </div>

          {recentAccounts.length > 0 && (
            <aside className={styles.recentSidebar}>
              <h4>最近使用</h4>
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
                          {account.type === 'microsoft' ? 'Microsoft' : account.type === 'offline' ? '离线' : '第三方'}
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
                            <span>修改</span>
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
                            <span>使用</span>
                          </button>
                        </div>
                      )}
                    </div>
                  </div>
                ))}
              </div>
              <p className={styles.recentHint}>悬停查看操作选项</p>
              
              {/* 未登录状态的添加账户按钮 */}
              {!showAddMenu ? (
                <button 
                  className={styles.addAccountBtn}
                  onClick={() => setShowAddMenu(true)}
                >
                  <PlusIcon />
                  <span>添加账户</span>
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
                    <span>离线模式</span>
                    <ChevronRightIcon />
                  </button>
                  <button onClick={() => { setShowAddMenu(false); setMethod('thirdparty'); }}>
                    <ServerIcon />
                    <span>第三方</span>
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
              <h3>Microsoft 账户登录</h3>
              <p>将在系统浏览器中打开微软官方授权页面</p>
            </div>
          </div>

          <div className={styles.infoBox}>
            <h4>登录流程</h4>
            <ol>
              <li>点击下方按钮后，将打开系统默认浏览器</li>
              <li>在微软官方页面输入账号密码</li>
              <li>授权 FlowLaunch 访问你的游戏信息</li>
              <li>完成后自动返回启动器</li>
            </ol>
          </div>

          <div className={styles.notice}>
            <ShieldCheckIcon />
            <p>FlowLaunch 不会获取或保存你的微软密码。授权令牌由系统安全存储。</p>
          </div>

          <div className={styles.actions}>
            <Button onClick={handleMicrosoftLogin}>
              通过微软登录（演示）
            </Button>
            <Button variant="ghost" onClick={resetView}>
              返回
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
              <h3>离线模式</h3>
              <p>输入一个游戏内显示的用户名</p>
            </div>
          </div>

          <Input
            label="用户名"
            placeholder="请输入用户名"
            value={offlineName}
            onChange={(e) => setOfflineName(e.target.value)}
            hint="3-16个字符，仅支持字母、数字和下划线"
          />

          <div className={`${styles.notice} ${styles.warning}`}>
            <AlertIcon />
            <div>
              <h4>离线模式限制</h4>
              <ul>
                <li>只能用于单人游戏或支持离线模式的服务器</li>
                <li>无法加入需要正版验证的服务器</li>
                <li>皮肤需要手动设置或使用服务器端插件</li>
                <li>不能替代正版游戏所有权验证</li>
              </ul>
            </div>
          </div>

          <div className={styles.actions}>
            <Button onClick={handleOfflineLogin} disabled={!offlineName.trim()}>
              使用离线模式
            </Button>
            <Button variant="ghost" onClick={resetView}>
              返回
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
              <h3>第三方认证服务器</h3>
              <p>使用 authlib-injector 或兼容认证服务</p>
            </div>
          </div>

          <Input
            label="认证服务器地址"
            placeholder="https://example.com/api/yggdrasil"
            value={serverUrl}
            onChange={(e) => setServerUrl(e.target.value)}
            hint="输入服务器提供的 Yggdrasil API 地址"
          />

          <div className={styles.infoBox}>
            <h4>什么是第三方认证？</h4>
            <p>一些组织（如学校、社区）会搭建自己的认证服务器，提供类似正版的验证和皮肤功能。你需要从服务器管理员获取：</p>
            <ul>
              <li>认证服务器 API 地址</li>
              <li>已注册的账户和密码</li>
            </ul>
          </div>

          <div className={`${styles.notice} ${styles.warning}`}>
            <AlertIcon />
            <div>
              <h4>安全提示</h4>
              <p>仅使用你信任的认证服务器。恶意服务器可能窃取账户信息。官方 Minecraft 不提供第三方认证支持。</p>
            </div>
          </div>

          <div className={styles.actions}>
            <Button onClick={handleThirdPartyLogin} disabled={!serverUrl.trim()}>
              连接认证服务器（演示）
            </Button>
            <Button variant="ghost" onClick={resetView}>
              返回
            </Button>
          </div>
        </div>
      )}
    </Modal>
  );
}
