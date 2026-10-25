import { AlertTriangle, CheckCircle2, FolderOpen, Terminal } from 'lucide-react';
import type { GameInstance } from '../types';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import {
  DEFAULT_INSTANCE_JAVA,
  formatJavaRange,
  javaRequirementFor,
  parseJavaRange,
  resolveJava,
  type InstanceJavaMode,
  type InstanceJavaSetting,
  type JavaRuntime,
} from '../utils/java';
import { renderLocalized } from '../utils/i18n/localized';
import { renderRequirement, renderRequirementNotes } from '../utils/java';
import { useI18n } from '../utils/i18n';
import styles from './InstanceJavaCard.module.css';

interface InstanceJavaCardProps {
  instance: GameInstance;
  runtimes: JavaRuntime[];
  setting: InstanceJavaSetting;
  globalPath: string;
  onChange: (setting: InstanceJavaSetting) => void;
  onManageJava: () => void;
  /**
   * 嵌在别处时隐藏自带标题。
   *
   * 实例管理页已经把这一段包在「Java 运行环境」分组里，再显示一遍图标 + 标题
   * 就成了同名两级标题；而独立页面（没有外层分组）仍然需要它当卡片标题。
   */
  embedded?: boolean;
}

/**
 * The four ways an instance can obtain its Java. These mirror PCL2's
 * "版本独立设置 → Java" combo box one to one, including the order of options, so
 * anyone coming from PCL finds the same vocabulary here.
 */
const modes: { id: InstanceJavaMode; labelKey: MessageKey; hintKey: MessageKey }[] = [
  { id: 'auto', labelKey: 'java.modeAuto', hintKey: 'java.modeAutoHint' },
  { id: 'range', labelKey: 'java.modeRange', hintKey: 'java.modeRangeHint' },
  { id: 'folder', labelKey: 'java.modeFolder', hintKey: 'java.modeFolderHint' },
  { id: 'forced', labelKey: 'java.modeForced', hintKey: 'java.modeForcedHint' },
];

export function InstanceJavaCard({ instance, runtimes, setting, globalPath, onChange, onManageJava, embedded = false }: InstanceJavaCardProps) {
  const { t, language } = useI18n();
  const requirement = javaRequirementFor(instance);
  const resolution = resolveJava({ setting, requirement, runtimes, globalPath, language });
  const selectedMode = modes.find(mode => mode.id === setting.mode) ?? modes[0];
  const rangeError = setting.mode === 'range' && setting.range.trim() ? parseJavaRange(setting.range) : null;
  const rangeMessage = rangeError && 'error' in rangeError ? renderLocalized(language, rangeError.error) : '';

  const update = (patch: Partial<InstanceJavaSetting>) => onChange({ ...DEFAULT_INSTANCE_JAVA, ...setting, ...patch });

  const toneClass = resolution.tone === 'ok' ? styles.ok : resolution.tone === 'error' ? styles.error : styles.warn;
  const ToneIcon = resolution.tone === 'ok' ? CheckCircle2 : AlertTriangle;

  return (
    <section className={styles.card}>
      {!embedded && (
        <div className={styles.heading}>
          <span className={styles.icon}><Terminal size={19} strokeWidth={1.8} /></span>
          <div className={styles.headingCopy}>
            <h2>{t('java.sectionTitle')}</h2>
            <p>{t('java.sectionHint')}</p>
          </div>
          <span className={styles.badge}>{t('java.instanceBadge')}</span>
        </div>
      )}

      <label className={styles.field}>
        <span>{t('java.thisInstanceUses')}</span>
        <select
          aria-label={t('java.modeAriaLabel', { values: { name: instance.name } })}
          value={setting.mode}
          onChange={event => update({ mode: event.target.value as InstanceJavaMode })}
        >
          {modes.map(mode => <option key={mode.id} value={mode.id}>{t(mode.labelKey)}</option>)}
        </select>
      </label>
      <p className={styles.fieldHint}>{t(selectedMode.hintKey)}</p>

      {setting.mode === 'range' && (
        <div className={styles.detail}>
          <label className={styles.detailField}>
            <span>{t('java.range')}</span>
            <input
              value={setting.range}
              onChange={event => update({ range: event.target.value })}
              placeholder={t('java.rangePlaceholder')}
              aria-label={t('java.rangeAriaLabel', { values: { name: instance.name } })}
              aria-invalid={Boolean(rangeMessage)}
            />
          </label>
          <p className={styles.detailHint}>
            {t('java.rangeHint', { values: { recommended: renderRequirement(requirement, language) } })}
            {rangeError && !('error' in rangeError) ? ` ${t('java.rangeParsed', { values: { range: formatJavaRange(rangeError) } })}` : ''}
          </p>
        </div>
      )}

      {setting.mode === 'folder' && (
        <div className={styles.detail}>
          <label className={styles.detailField}>
            <span>{t('java.folder')}</span>
            <input
              value={setting.folder}
              onChange={event => update({ folder: event.target.value })}
              placeholder={t('java.folderPlaceholder', { values: { name: instance.name } })}
              aria-label={t('java.folderAriaLabel', { values: { name: instance.name } })}
            />
          </label>
          <p className={styles.detailHint}>
            <FolderOpen size={12} /> {t('java.folderPreview')}
          </p>
        </div>
      )}

      {setting.mode === 'forced' && (
        <div className={styles.detail}>
          <label className={styles.detailField}>
            <span>{t('java.forced')}</span>
            <select
              aria-label={t('java.forcedAriaLabel', { values: { name: instance.name } })}
              value={setting.path}
              onChange={event => update({ path: event.target.value })}
            >
              <option value="">{runtimes.length ? t('java.chooseJava') : t('java.importFirst')}</option>
              {runtimes.map((runtime, index) => (
                <option key={runtime.path} value={runtime.path}>
                  {index === 0 ? t('java.priorityTag') : ''}Java {runtime.majorVersion} · {runtime.versionText} · {runtime.path}
                </option>
              ))}
            </select>
          </label>
        </div>
      )}

      <div className={`${styles.status} ${toneClass}`}>
        <ToneIcon size={15} strokeWidth={2} />
        <div>
          <strong>
            {rangeMessage
              ? t('java.rangeError', { values: { message: rangeMessage } })
              : renderLocalized(language, resolution.message)}
          </strong>
          <small>
            {requirement.notes.length
              ? t('java.requirementNotes', { values: { requirement: renderRequirement(requirement, language), notes: renderRequirementNotes(requirement, language) } })
              : t('java.requirement', { values: { requirement: renderRequirement(requirement, language) } })}
          </small>
        </div>
      </div>

      <div className={styles.actions}>
        <span>{t('java.listSummary', { count: runtimes.length })}</span>
        <button type="button" className={styles.manage} onClick={onManageJava}>{t('java.manageList')}</button>
      </div>
    </section>
  );
}
