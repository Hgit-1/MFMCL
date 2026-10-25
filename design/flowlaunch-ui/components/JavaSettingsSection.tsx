import { ArrowDown, ArrowUp, FolderOpen, Plus, Search, Trash2, Terminal } from 'lucide-react';
import type { GameInstance } from '../types';
import { InstanceJavaCard } from './InstanceJavaCard';
import {
  formatRuntimeShort,
  moveJava,
  runtimeFolder,
  type InstanceJavaSetting,
  type JavaRuntime,
} from '../utils/java';
import { renderLocalized, type Localized } from '../utils/i18n/localized';
import { useI18n } from '../utils/i18n';
import styles from './JavaSettingsSection.module.css';

interface JavaSettingsSectionProps {
  runtimes: JavaRuntime[];
  globalPath: string;
  /** 状态文案，可以是词典键或已经渲染好的字符串。 */
  status: Localized;
  busy?: boolean;
  instance?: GameInstance;
  instanceSetting: InstanceJavaSetting;
  onRuntimesChange: (runtimes: JavaRuntime[]) => void;
  onGlobalPathChange: (path: string) => void;
  onInstanceSettingChange: (setting: InstanceJavaSetting) => void;
  onDetect: () => void;
  onImport: () => void;
  onReveal: (runtime: JavaRuntime) => void;
}

/**
 * Global Java settings, laid out like PCL2's 设置 → 启动 → Java 列表:
 * one ordered list, ordering = priority, plus search / import / remove / reveal.
 * Instance-level choices live in <InstanceJavaCard>, matching PCL2's split between
 * 全局设置 and 版本独立设置.
 */
export function JavaSettingsSection({
  runtimes,
  globalPath,
  status,
  busy = false,
  instance,
  instanceSetting,
  onRuntimesChange,
  onGlobalPathChange,
  onInstanceSettingChange,
  onDetect,
  onImport,
  onReveal,
}: JavaSettingsSectionProps) {
  const { t, language } = useI18n();
  const move = (index: number, offset: number) => onRuntimesChange(moveJava(runtimes, index, offset));
  const remove = (runtime: JavaRuntime) => {
    onRuntimesChange(runtimes.filter(item => item.path !== runtime.path));
    if (globalPath === runtime.path) onGlobalPathChange('');
  };

  return (
    <>
      {instance && (
        <section className={styles.instanceGroup}>
          <InstanceJavaCard
            instance={instance}
            runtimes={runtimes}
            setting={instanceSetting}
            globalPath={globalPath}
            onChange={onInstanceSettingChange}
            onManageJava={() => {
              document.querySelector<HTMLElement>(`[data-java-list]`)?.scrollIntoView({ behavior: 'smooth', block: 'center' });
            }}
          />
        </section>
      )}

      <section className={styles.group}>
        <h3>{t('java.listTitle')}</h3>
        <div className={styles.toolbar}>
          <div className={styles.status}>
            <Terminal size={15} strokeWidth={1.9} />
            <span>{busy ? t('java.searching') : renderLocalized(language, status)}</span>
          </div>
          <div className={styles.toolbarButtons}>
            <button type="button" className={styles.action} onClick={onDetect} disabled={busy}>
              <Search size={14} /> {t('java.autoDetect')}
            </button>
            <button type="button" className={styles.action} onClick={onImport} disabled={busy}>
              <Plus size={14} /> {t('java.importHtml')}
            </button>
          </div>
        </div>

        {runtimes.length === 0 ? (
          <p className={styles.empty}>{t('java.emptyList')}</p>
        ) : (
          <ul className={styles.list} aria-label={t('java.listAriaLabel')} data-java-list>
            {runtimes.map((runtime, index) => {
              const isGlobal = globalPath === runtime.path;
              return (
                <li key={runtime.path} className={styles.item}>
                  <span className={styles.index}>{index + 1}</span>
                  <div className={styles.itemCopy}>
                    <div className={styles.itemTitle}>
                      <b>Java {runtime.majorVersion}</b>
                      <span className={styles.itemVersion}>{runtime.versionText}</span>
                      {index === 0 && <em className={styles.priority}>{t('java.priority')}</em>}
                      {isGlobal && <em className={styles.assigned}>{t('java.globalDefault')}</em>}
                    </div>
                    <small title={runtime.path}>{runtime.path}</small>
                  </div>
                  <div className={styles.itemActions}>
                    <button
                      type="button"
                      className={styles.iconButton}
                      onClick={() => move(index, -1)}
                      disabled={index === 0}
                      aria-label={t('java.raiseAria', { values: { folder: runtimeFolder(runtime) } })}
                      title={t('java.raisePriority')}
                    >
                      <ArrowUp size={14} />
                    </button>
                    <button
                      type="button"
                      className={styles.iconButton}
                      onClick={() => move(index, 1)}
                      disabled={index === runtimes.length - 1}
                      aria-label={t('java.lowerAria', { values: { folder: runtimeFolder(runtime) } })}
                      title={t('java.lowerPriority')}
                    >
                      <ArrowDown size={14} />
                    </button>
                    <button
                      type="button"
                      className={styles.iconButton}
                      onClick={() => onReveal(runtime)}
                      aria-label={t('java.openAria', { values: { folder: runtimeFolder(runtime) } })}
                      title={t('java.openFolder')}
                    >
                      <FolderOpen size={14} />
                    </button>
                    <button
                      type="button"
                      className={`${styles.iconButton} ${styles.dangerButton}`}
                      onClick={() => remove(runtime)}
                      aria-label={t('java.removeAria', { values: { folder: runtimeFolder(runtime) } })}
                      title={t('java.removeFromList')}
                    >
                      <Trash2 size={14} />
                    </button>
                  </div>
                </li>
              );
            })}
          </ul>
        )}

        <p className={styles.note}>
          {t('java.orderingNote')}
        </p>

        <label className={styles.globalField}>
          <span>{t('java.globalDefaultLabel')}</span>
          <select value={globalPath} onChange={event => onGlobalPathChange(event.target.value)}>
            <option value="">{t('java.globalAuto')}</option>
            {runtimes.map(runtime => (
              <option key={runtime.path} value={runtime.path}>
                {formatRuntimeShort(runtime)} · {runtime.path}
              </option>
            ))}
          </select>
        </label>
        <p className={styles.note}>
          {t('java.globalNote')}
        </p>
      </section>

    </>
  );
}
