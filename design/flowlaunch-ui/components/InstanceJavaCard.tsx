import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { AlertTriangle, CheckCircle2, FolderOpen, Terminal } from 'lucide-react';
import type { GameInstance } from '../types';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import {
  DEFAULT_INSTANCE_JAVA,
  describeJavaRange,
  javaRequirementFor,
  javaSelectionRequest,
  javaRuleForPersistence,
  formatRuntime,
  parseFriendlyRange,
  resolveJava,
  type InstanceJavaMode,
  type InstanceJavaSetting,
  type JavaRuntime,
  type NativeJavaSelection,
  type JavaRequirement,
  type JavaResolution,
} from '../utils/java';
import {
  flushPendingJavaPreferences, getJavaRuleDraft, loadJavaPreferences, selectInstanceJava,
  stageJavaRuleDraft, subscribeJavaRuleDraft, syncConfirmedJavaRule,
} from '../utils/javaNative';
import { isDesktopShell } from '../utils/native';
import { loc, renderLocalized } from '../utils/i18n/localized';
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

export function InstanceJavaCard({ instance, runtimes, setting: suppliedSetting, globalPath, onChange, onManageJava, embedded = false }: InstanceJavaCardProps) {
  const { t, language } = useI18n();
  const immediate = embedded && isDesktopShell();
  const draft = useSyncExternalStore(
    listener => immediate ? subscribeJavaRuleDraft(instance.id, listener) : () => undefined,
    () => getJavaRuleDraft(instance.id),
  );
  const [persistence, setPersistence] = useState<{ id: string; ready: boolean; error?: string }>({ id: instance.id, ready: !immediate });
  const callbackRef = useRef(onChange);
  callbackRef.current = onChange;
  const suppliedKey = JSON.stringify(javaRuleForPersistence(suppliedSetting));
  useEffect(() => {
    if (immediate) syncConfirmedJavaRule(instance.id, JSON.parse(suppliedKey));
  }, [immediate, instance.id, suppliedKey]);
  useEffect(() => {
    if (!immediate) return;
    let active = true;
    setPersistence({ id: instance.id, ready: false });
    void loadJavaPreferences().then(() => {
      if (active) setPersistence({ id: instance.id, ready: true });
    }, error => {
      if (active) setPersistence({ id: instance.id, ready: getJavaRuleDraft(instance.id).dirty, error: String(error) });
    });
    return () => {
      active = false;
      // The shared draft and its error survive unmount, even when this fire-and-forget flush fails.
      void flushPendingJavaPreferences(instance.id).catch(() => undefined);
    };
  }, [immediate, instance.id]);
  useEffect(() => {
    if (immediate && persistence.id === instance.id && persistence.ready && !draft.dirty) {
      callbackRef.current(draft.confirmed);
    }
  }, [immediate, instance.id, persistence.id, persistence.ready, draft]);
  const setting = immediate ? draft.rule : suppliedSetting;
  const persistenceError = immediate ? draft.error ?? (persistence.id === instance.id ? persistence.error : undefined) : undefined;
  const blocked = immediate && (persistence.id !== instance.id || !persistence.ready);
  const requestKey = JSON.stringify(javaSelectionRequest(setting, runtimes, globalPath));
  const selectionKey = `${instance.id}:${instance.version}:${instance.loader}:${instance.loaderVersion}:${requestKey}`;
  const [native, setNative] = useState<{ key: string; result?: NativeJavaSelection; error?: string } | null>(null);
  useEffect(() => {
    if (!isDesktopShell()) return;
    let active = true;
    const timer = window.setTimeout(() => {
      void selectInstanceJava(instance.id, JSON.parse(requestKey)).then(
        result => { if (active) setNative({ key: selectionKey, result }); },
        error => { if (active) setNative({ key: selectionKey, error: String(error) }); },
      );
    }, 150);
    return () => { active = false; window.clearTimeout(timer); };
  }, [instance.id, requestKey, selectionKey]);
  const current = native?.key === selectionKey ? native : null;
  const requirement: JavaRequirement = current?.result
    ? { ...current.result.requirement, notes: [] }
    : javaRequirementFor(instance);
  const resolution: JavaResolution = persistenceError
    ? { runtime: null, tone: 'error', message: persistenceError }
    : !isDesktopShell()
    ? resolveJava({ setting, requirement, runtimes, globalPath, language })
    : current?.result?.runtime
      ? { runtime: current.result.runtime, tone: 'ok', message: loc('java.willUse', { runtime: formatRuntime(current.result.runtime) }) }
      : current?.error
        ? { runtime: null, tone: 'error', message: current.error }
        : current?.result?.canAutoInstall
          ? { runtime: null, tone: 'ok', message: loc('java.noCompatible', { requirement: renderRequirement(requirement, language) }) }
          : { runtime: null, tone: 'warn', message: loc('java.searching') };
  const selectedMode = modes.find(mode => mode.id === setting.mode) ?? modes[0];
  // 用友好解析器：用户写 `21`、`21+`、`17-21` 都能理解，
  // 旧的 `[21.0, )` 语法也仍然接受（已保存的设置里存的就是这种）。
  const rangeError = setting.mode === 'range' && setting.range.trim() ? parseFriendlyRange(setting.range) : null;
  const rangeMessage = rangeError && 'error' in rangeError ? renderLocalized(language, rangeError.error) : '';

  const update = (patch: Partial<InstanceJavaSetting>) => {
    if (blocked) return;
    const next = javaRuleForPersistence({ ...DEFAULT_INSTANCE_JAVA, ...setting, ...patch });
    if (!immediate) { onChange(next); return; }
    stageJavaRuleDraft(instance.id, next);
    setPersistence({ id: instance.id, ready: true });
  };

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
          {/*
            徽标直接写实例名，而不是「实例设置」。
            「实例设置」只说明了这是实例级配置，却没有回答最要紧的问题：哪一个实例？
            设置面板是覆盖在页面上的一层，背后的实例页可能已被遮挡，用户无法从
            徽标本身推断作用对象。
          */}
          <span className={styles.badge} title={instance.name}>{instance.name}</span>
        </div>
      )}

      {/*
        嵌入模式没有标题行，这里单独用一条标签写明作用对象。
        无论哪种模式，「这个实例使用…」里的 "this instance" 都必须有明确前指对象；
        靠用户记住自己是从哪个页面点进来的不算说明。
      */}
      {embedded && (
        <div className={styles.instanceTag}>
          <span className={styles.instanceTagLabel}>{t('java.scopeLabel')}</span>
          <strong className={styles.instanceTagName} title={instance.name}>{instance.name}</strong>
        </div>
      )}

      <label className={styles.field}>
        {/*
          有徽标时标签保持简洁——实例名已经在上方写明，两处都写会让这一行
          变长，读起来像两件事。
          嵌入模式没有徽标（标题行被隐藏），所以标签必须自带实例名，
          否则 "this instance" 没有前指对象。
        */}
        <span>{embedded ? t('java.usesForInstance', { values: { name: instance.name } }) : t('java.thisInstanceUses')}</span>
        <select
          aria-label={t('java.modeAriaLabel', { values: { name: instance.name } })}
          value={setting.mode}
          disabled={blocked}
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
          </p>
          {/*
            把解析结果翻译回人能读的一句话。
            原来这里显示 `[21.0, )`——用户刚刚就是因为看不懂这个写法才改成自然语法的，
            再用它做「解析结果」等于把人推回原地。所以改成「Java 21 及更高版本」。
            无效时显示具体原因（哪一边写错了），而不是笼统的「格式错误」。
          */}
          {rangeError && 'error' in rangeError && (
            <p className={styles.detailError}>{renderLocalized(language, rangeError.error)}</p>
          )}
          {rangeError && !('error' in rangeError) && (
            <p className={styles.detailParsed}>
              {t('java.rangeUnderstood', { values: { reading: describeJavaRange(rangeError, language) } })}
            </p>
          )}
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
