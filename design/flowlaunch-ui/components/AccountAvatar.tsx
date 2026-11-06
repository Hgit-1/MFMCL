// 账户头像（侧栏与账户弹窗共用）。
//
// ## 这个组件解决的问题
//
// 头像来源有两处，之前各自实现、结果不一致：
//
//   - 侧栏（`App.tsx` 的 `SidebarAccountAvatar`）：先看账户自带的 `avatar`，没有
//     再用设置里上传的离线皮肤。
//   - 账户弹窗（`AccountModal.tsx` 的 `PlayerAvatar`）：只看账户自带的 `avatar`，
//     离线档案一律显示灰色人形。
//
// 于是同一个离线档案在侧栏里是玩家上传的皮肤、在账户弹窗里是灰色占位——用户会
// 以为自己上传的皮肤没生效，或者以为两处显示的是不同账户。
//
// 这里把「用哪张图」的规则收在一处，两个界面都调它，显示必然一致。
//
// ## 优先级
//
// 1. 账户自带的 `avatar`（正版 / 皮肤站登录时由后端给的地址）；
// 2. 本地离线皮肤（设置 → 外观 → 离线皮肤里上传的 PNG）；
// 3. 默认人形占位。
//
// 第 1 项优先是必须的：正版账户的头像是它的真实身份，不能被本地覆盖图顶掉。
// 而离线档案没有服务器皮肤，本地图就是它唯一的形象来源。

import { useEffect, useState } from 'react';
import { useI18n } from '../utils/i18n';
import { renderSkinAvatar, skinSourceOf } from '../utils/skinAvatar';
import { DefaultUserAvatar } from './DefaultUserAvatar';
import type { SkinSetting } from './SkinSection';
import type { Account } from '../types';

export interface AccountAvatarProps {
  /** 当前账户。为 `undefined` 时表示未登录，只显示占位。 */
  account?: Account;
  /** 设置里的离线皮肤；离线档案在没有自带头像时用它渲染。 */
  skin?: SkinSetting;
  /** 占位图与图片的类名，由调用方按各自布局给出尺寸。 */
  imageClassName?: string;
  fallbackClassName?: string;
}

/**
 * 一个账户的头像。
 *
 * 渲染过程是异步的：本地皮肤要先从 PNG 里裁出脸。裁剪尚未完成时先显示占位，
 * 完成后替换——不阻塞布局，也不会出现「一会儿有图一会儿没图」的跳动，因为占位
 * 与结果占据同一个盒子。
 */
export function AccountAvatar({
  account,
  skin,
  imageClassName,
  fallbackClassName,
}: AccountAvatarProps) {
  const { t } = useI18n();
  const [rendered, setRendered] = useState<string | null>(null);

  /*
   * 本地皮肤只作用于「没有服务器头像」的账户。
   *
   * 具体是两类：
   *   - 离线档案：它没有服务器皮肤，用户上传的那张 PNG 就是唯一的形象来源；
   *   - 未登录：没有账户可言，显示用户为自己选的形象是合理的（侧栏一直是这么做的）。
   *
   * 正版 / 皮肤站账户即使设置了离线皮肤（那是给离线启动准备的），也不该让启动器
   * 里显示一个与自己在游戏里不符的头像。这条规则也与「离线皮肤只作用于离线启动」
   * 保持一致。
   */
  const usesLocalSkin = !account || account.type === 'offline';
  const localSkin = usesLocalSkin && skin ? skinSourceOf(skin) : null;
  const source = account?.avatar ?? localSkin;

  useEffect(() => {
    let alive = true;
    if (!source) {
      setRendered(null);
      return;
    }
    void renderSkinAvatar({ source }).then(result => {
      if (alive) setRendered(result);
    });
    return () => { alive = false; };
  }, [source]);

  const alt = account
    ? t('account.avatarAlt', { values: { name: account.username } })
    : t('account.avatarPlaceholder', { values: { name: '' } });

  if (rendered) {
    return (
      <img
        className={imageClassName}
        src={rendered}
        alt={alt}
        title={account?.username}
      />
    );
  }
  return (
    <DefaultUserAvatar
      className={fallbackClassName}
      label={alt}
    />
  );
}
