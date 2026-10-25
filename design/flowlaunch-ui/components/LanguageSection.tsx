// 语言选择器。
//
// 放在设置面板「外观」页，而不是做成侧栏的独立开关：语言是低频设置，和主题、
// 强调色同属「外观」，放在一起比多一个侧栏图标更省空间。
//
// 用分段按钮而不是下拉框：只有四个选项，全展示出来可以避免用户多点一次，
// 也让「跟随系统」这个状态一眼可见。

import { Check, Languages } from 'lucide-react';
import { LANGUAGES, useI18n } from '../utils/i18n';
import type { Language, LanguagePreference } from '../utils/i18n';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import styles from './LanguageSection.module.css';

const LABEL_KEYS: Record<LanguagePreference, MessageKey> = {
  system: 'language.system',
  'zh-CN': 'language.zh',
  en: 'language.en',
  ja: 'language.ja',
};

export function LanguageSection() {
  const { language, preference, setLanguage, t } = useI18n();
  const nativeOf = (id: LanguagePreference | Language) => LANGUAGES.find(item => item.id === id)?.native ?? id;
  // 跟随系统时同时显示实际生效的语言，否则「当前：跟随系统」等于没说。
  const currentLabel = preference === 'system'
    ? `${t('language.system')} · ${nativeOf(language)}`
    : nativeOf(language);

  return (
    <section className={styles.group}>
      <h3>{t('language.title')}</h3>
      <div className={styles.row}>
        <div className={styles.copy}>
          <span className={styles.icon} aria-hidden="true"><Languages size={16} /></span>
          <div>
            <strong>{t('language.title')}</strong>
            <span>{t('language.description')}</span>
          </div>
        </div>
        <div className={styles.options} role="radiogroup" aria-label={t('language.title')}>
          {LANGUAGES.map(item => {
            const active = preference === item.id;
            const label = t(LABEL_KEYS[item.id]);
            return (
              <button
                key={item.id}
                type="button"
                role="radio"
                aria-checked={active}
                className={active ? styles.active : ''}
                onClick={() => setLanguage(item.id)}
                lang={item.id === 'system' ? undefined : item.id}
              >
                <span>{label}</span>
                {active && <Check size={13} aria-hidden="true" />}
              </button>
            );
          })}
        </div>
      </div>
      <p className={styles.current}>
        {t('language.current', { values: { language: currentLabel } })}
      </p>
    </section>
  );
}
