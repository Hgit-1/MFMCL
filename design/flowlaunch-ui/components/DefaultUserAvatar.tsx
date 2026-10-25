import { useId } from 'react';

import { useI18n } from '../utils/i18n';

// 通用用户头像占位符：未登录或账户没有皮肤头像时使用。
// 造型采用业界通用的“中性圆底 + 人形剪影”，肩部延伸到圆外并由圆形裁切，
// 因此底边始终是平滑圆弧；在 20px 侧栏图标与 80px 账户预览下都清晰。
// 颜色跟随 currentColor，自动适配浅色 / 深色主题。
export function DefaultUserAvatar({ className, label }: { className?: string; label?: string }) {
  const { t } = useI18n();
  const clipId = `avatar-clip-${useId().replace(/:/g, '')}`;
  const accessibleLabel = label ?? t('account.defaultAvatarLabel');
  return (
    <svg className={className} viewBox="0 0 24 24" role="img" aria-label={accessibleLabel} focusable="false">
      <defs>
        <clipPath id={clipId}>
          <circle cx="12" cy="12" r="11" />
        </clipPath>
      </defs>
      <circle cx="12" cy="12" r="11" fill="currentColor" opacity="0.12" />
      <g clipPath={`url(#${clipId})`}>
        <circle cx="12" cy="9.2" r="3.7" fill="currentColor" opacity="0.74" />
        <path
          d="M12 14.4c-3.9 0-7.05 2.35-7.05 5.25V26h14.1v-6.35c0-2.9-3.15-5.25-7.05-5.25Z"
          fill="currentColor"
          opacity="0.74"
        />
      </g>
      <circle cx="12" cy="12" r="11" fill="none" stroke="currentColor" strokeOpacity="0.18" strokeWidth="1" />
    </svg>
  );
}
