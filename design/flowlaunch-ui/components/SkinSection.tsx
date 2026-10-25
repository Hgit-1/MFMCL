// 离线皮肤（PCL2 的「离线皮肤」卡片）。
//
// 这里原本放的是一个并不存在的「自定义皮肤 / PCL / 自定义 Yggdrasil」下拉框，
// 「PCL」这个选项没有任何含义，所以整体替换为 PCL2 真正的实现：
//
//   PCL/Plain Craft Launcher 2/Pages/PageSetup/PageSetupLaunch.xaml（离线皮肤卡片）
//   五个选项：随机 / Steve / Alex / 正版皮肤 / 自定义
//   - 正版皮肤：填一个正版玩家名，PCL 用它的 UUID 让游戏去正版皮肤服务器取皮肤
//     （McLoginLegacyUuidWithCustomSkin，Case 3）。
//   - 自定义：把 PNG 当资源包加载，只对自己可见，并且会替换游戏里所有角色
//     （Case 4 + ModLaunch 里的「离线皮肤资源包」）。
//   - 随机 / Steve / Alex：靠调整离线 UUID 的后五位，让游戏的默认皮肤
//     落在对应模型上（McSkinSex）。
//
// PCL 自己标注了这个功能的局限：只保证对 1.19.2 以前的版本有效。这一条也要
// 如实显示，不能把它说成通用能力。

import { useEffect, useRef, useState } from 'react';
import { AlertTriangle, ImagePlus, RefreshCw, Trash2 } from 'lucide-react';
import { useI18n } from '../utils/i18n';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import styles from './SkinSection.module.css';

export type SkinType = 'random' | 'steve' | 'alex' | 'premium' | 'custom';

export interface SkinSetting {
  type: SkinType;
  /** 正版皮肤：目标玩家名。 */
  premiumName: string;
  /** 自定义皮肤：粗臂（Steve）还是细臂（Alex）。 */
  slim: boolean;
  /** 自定义皮肤：已经通过校验的 PNG（data URL），仅本地预览用。 */
  customDataUrl: string;
  /** 自定义皮肤的文件名，用于在界面上显示当前选中的文件。 */
  customFileName: string;
}

export const DEFAULT_SKIN_SETTING: SkinSetting = {
  type: 'random',
  premiumName: '',
  slim: false,
  customDataUrl: '',
  customFileName: '',
};

const SKIN_MODES: { id: SkinType; labelKey: MessageKey; hintKey: MessageKey }[] = [
  { id: 'random', labelKey: 'skin.typeRandom', hintKey: 'skin.hintRandom' },
  { id: 'steve', labelKey: 'skin.typeSteve', hintKey: 'skin.hintSteve' },
  { id: 'alex', labelKey: 'skin.typeAlex', hintKey: 'skin.hintAlex' },
  { id: 'premium', labelKey: 'skin.typePremium', hintKey: 'skin.hintPremium' },
  { id: 'custom', labelKey: 'skin.typeCustom', hintKey: 'skin.hintCustom' },
];

/** PCL 的校验规则：64×32 或 64×64，且小于 24 KB。 */
const MIN_SIDE = 64;
const MAX_BYTES = 24 * 1024;

interface SkinSectionProps {
  setting: SkinSetting;
  onChange: (setting: SkinSetting) => void;
}

export function SkinSection({ setting, onChange }: SkinSectionProps) {
  const { t } = useI18n();
  // 错误存成「键 + 参数」而不是成品字符串，切换语言后提示也会跟着变。
  const [error, setError] = useState<{ key: MessageKey; values?: Record<string, string | number> } | null>(null);
  const [busy, setBusy] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);

  // 切换到非自定义模式时清掉错误，避免旧的校验失败信息一直挂着。
  useEffect(() => {
    if (setting.type !== 'custom') setError(null);
  }, [setting.type]);

  const patch = (next: Partial<SkinSetting>) => onChange({ ...setting, ...next });

  const pickFile = async (file: File | undefined) => {
    if (!file) return;
    setError(null);
    setBusy(true);
    try {
      if (file.size > MAX_BYTES) {
        setError({ key: 'skin.errTooLarge', values: { size: `${(file.size / 1024).toFixed(2)} KB` } });
        return;
      }
      const dataUrl = await readAsDataUrl(file);
      const size = await imageSize(dataUrl);
      if (!size) {
        setError({ key: 'skin.errUnreadable' });
        return;
      }
      if (size.width !== MIN_SIDE || (size.height !== 32 && size.height !== 64)) {
        setError({ key: 'skin.errBadSize', values: { size: `${size.width}×${size.height}` } });
        return;
      }
      // 单层皮肤（64×32）在 PCL 里会被扩展到 64×64，这里只记录尺寸供提示。
      patch({ customDataUrl: dataUrl, customFileName: file.name });
    } catch {
      // FileReader 失败（文件被占用、权限不足等）也必须给用户可见的反馈，
      // 否则选择文件后界面毫无反应。
      setError({ key: 'skin.readFailed' });
    } finally {
      setBusy(false);
      if (fileRef.current) fileRef.current.value = '';
    }
  };

  return (
    <section className={styles.group}>
      <h3>{t('skin.title')}</h3>

      <p className={styles.warning} role="note">
        <AlertTriangle size={13} />
        {t('skin.limitedWarning')}
      </p>

      <div className={styles.modes} role="radiogroup" aria-label={t('skin.title')}>
        {SKIN_MODES.map(mode => {
          const active = setting.type === mode.id;
          return (
            <button
              key={mode.id}
              type="button"
              role="radio"
              aria-checked={active}
              className={`${styles.mode} ${active ? styles.modeActive : ''}`}
              onClick={() => patch({ type: mode.id })}
              title={t(mode.hintKey)}
            >
              <span className={styles.modeLabel}>{t(mode.labelKey)}</span>
              <span className={styles.modeHint}>{t(mode.hintKey)}</span>
            </button>
          );
        })}
      </div>

      {setting.type === 'premium' && (
        <div className={styles.panel}>
          <label className={styles.field} htmlFor="skin-premium-name">
            <span>{t('skin.premiumName')}</span>
            <input
              id="skin-premium-name"
              value={setting.premiumName}
              maxLength={16}
              onChange={event => patch({ premiumName: event.target.value })}
              placeholder={t('skin.premiumPlaceholder')}
              autoComplete="off"
              spellCheck={false}
            />
          </label>
          <p className={styles.note}>
            {t('skin.premiumHint')}
          </p>
        </div>
      )}

      {setting.type === 'custom' && (
        <div className={styles.panel}>
          <div className={styles.previewRow}>
            <div className={styles.preview}>
              {setting.customDataUrl ? (
                <img
                  src={setting.customDataUrl}
                  alt={t('skin.previewAlt')}
                  className={styles.previewImage}
                />
              ) : (
                <ImagePlus size={22} className={styles.previewEmpty} />
              )}
            </div>
            <div className={styles.previewCopy}>
              <strong>{setting.customFileName || t('skin.fileChosen')}</strong>
              <span>{t('skin.fileHint')}</span>
            </div>
            <div className={styles.previewActions}>
              <button type="button" className={styles.button} onClick={() => fileRef.current?.click()} disabled={busy}>
                {setting.customDataUrl ? <RefreshCw size={13} /> : <ImagePlus size={13} />}
                {busy ? t('skin.reading') : setting.customDataUrl ? t('skin.changeSkin') : t('skin.chooseSkin')}
              </button>
              {setting.customDataUrl && (
                <button
                  type="button"
                  className={`${styles.button} ${styles.danger}`}
                  onClick={() => patch({ customDataUrl: '', customFileName: '' })}
                >
                  <Trash2 size={13} />{t('skin.resetSkin')}
                </button>
              )}
            </div>
            <input
              ref={fileRef}
              type="file"
              accept="image/png"
              className={styles.hiddenInput}
              onChange={event => void pickFile(event.target.files?.[0])}
            />
          </div>

          <div className={styles.slimRow}>
            <span>{t('skin.armModel')}</span>
            <div className={styles.slimOptions} role="radiogroup" aria-label={t('skin.armModel')}>
              {([[false, t('skin.slimSteve')], [true, t('skin.slimAlex')]] as const).map(([value, label]) => (
                <button
                  key={String(value)}
                  type="button"
                  role="radio"
                  aria-checked={setting.slim === value}
                  className={setting.slim === value ? styles.slimActive : ''}
                  onClick={() => patch({ slim: value })}
                >
                  {label}
                </button>
              ))}
            </div>
          </div>

          {error && <p className={styles.error} role="alert">{t(error.key, { values: error.values })}</p>}
          <p className={styles.note}>
            {t('skin.customNote')}
          </p>
        </div>
      )}

      {setting.type === 'random' && (
        <p className={styles.note}>{t('skin.randomNote')}</p>
      )}
    </section>
  );
}

function readAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(new Error('read-failed'));
    reader.readAsDataURL(file);
  });
}

function imageSize(src: string): Promise<{ width: number; height: number } | null> {
  return new Promise(resolve => {
    const image = new Image();
    image.onload = () => resolve({ width: image.naturalWidth, height: image.naturalHeight });
    image.onerror = () => resolve(null);
    image.src = src;
  });
}
