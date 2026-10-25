// 实例管理页。
//
// 这个页面回答的问题是「这个实例和全局有什么不一样」。所以每一项都先给出
// 全局当前值，再由用户决定是否覆盖——默认全部跟随全局，未覆盖时页面看起来
// 就是一张「与全局一致」的清单，而不是逼用户重新做一遍设置。
//
// 控件选择：
//   - 布尔项用三档（关 / 跟随全局 / 开）。两档开关无法表达「不设置」，
//     用户会分不清「这是全局的值」还是「我在这里设成了这个值」。
//   - 值项用两档（跟随全局 / 自定义），只有选中自定义时才展开具体输入框。
//     展开而不是常驻，是为了让默认状态足够安静：一屏能看到所有可覆盖项，
//     不必滚过一堆灰掉的输入框。
//   - 两种控件都由 SegmentedChoice 渲染，保证同一列里的高度、圆角、字号一致。

import { useMemo } from 'react';
import {
  Coffee,
  MemoryStick,
  Monitor,
  Play,
  Rocket,
  RotateCcw,
  Settings2,
  SlidersHorizontal,
} from 'lucide-react';
import type { GameInstance } from '../types';
import { useI18n } from '../utils/i18n';
import type { LauncherPrefs } from '../utils/launcherPrefs';
import {
  autoAllocateMb,
  deviceMemoryMbSync,
  gbToSliderIndex,
  lastKnownUsedMb,
  maxSliderIndexForTotal,
  MIN_SLIDER_INDEX,
  MIN_MEMORY_MB,
  sliderIndexToGb,
} from '../utils/memory';
import {
  clearOverride,
  countOverrides,
  type InstanceOverrides,
} from '../utils/instanceOverrides';
import {
  DEFAULT_INSTANCE_JAVA,
  type InstanceJavaSetting,
  type JavaRuntime,
} from '../utils/java';
import { InstanceJavaCard } from './InstanceJavaCard';
import { SegmentedChoice } from './SegmentedChoice';
import { TriStateSwitch } from './TriStateSwitch';
import styles from './InstanceSettings.module.css';

interface InstanceSettingsProps {
  instance: GameInstance;
  /** 全局设置，用于在「跟随全局」时显示当前生效值。 */
  global: LauncherPrefs;
  overrides: InstanceOverrides;
  onChange: (next: InstanceOverrides) => void;
  onBack: () => void;
  onLaunch: () => void;
  onOpenGlobal: () => void;
  /** Java 部分仍在全局设置里管理列表，这里只展示实例的选择。 */
  javaRuntimes: JavaRuntime[];
  javaSetting: InstanceJavaSetting;
  globalJavaPath: string;
  onJavaSettingChange: (setting: InstanceJavaSetting) => void;
  onManageJava: () => void;
}

function formatGb(mb: number): string {
  return `${(mb / 1024).toFixed(1)} GB`;
}

export function InstanceSettings({
  instance,
  global,
  overrides,
  onChange,
  onBack,
  onLaunch,
  onOpenGlobal,
  javaRuntimes,
  javaSetting,
  globalJavaPath,
  onJavaSettingChange,
  onManageJava,
}: InstanceSettingsProps) {
  const { t } = useI18n();

  // 设备内存只读一次，避免每次输入尺寸都触发一次探测。
  const totalGb = useMemo(() => Math.max(2, deviceMemoryMbSync() / 1024), []);
  const maxIndex = Math.max(MIN_SLIDER_INDEX + 1, maxSliderIndexForTotal(totalGb));
  const autoMb = useMemo(
    () => autoAllocateMb({ totalMb: deviceMemoryMbSync(), modCount: instance.modCount, usedMb: lastKnownUsedMb() }),
    [instance.modCount],
  );

  /**
   * 一次写入多个字段。
   *
   * 必须合并成一次 onChange：分两次调用时，第二次拿到的还是本次渲染的
   * overrides（不含第一次的改动），后写的那次会把先写的覆盖掉。拖内存滑条
   * 同时要设 memoryMode 和 memoryMb，正好踩到这个坑。
   */
  const patch = (partial: Partial<InstanceOverrides>) => {
    const next = { ...overrides };
    (Object.keys(partial) as (keyof InstanceOverrides)[]).forEach(key => {
      const value = partial[key];
      if (value === undefined) delete next[key];
      else (next as Record<string, unknown>)[key] = value;
    });
    onChange(next);
  };

  const set = <K extends keyof InstanceOverrides>(key: K, value: InstanceOverrides[K]) => {
    patch({ [key]: value } as Partial<InstanceOverrides>);
  };

  /**
   * 切换内存模式。
   *
   * 离开「自定义」时顺手丢掉 memoryMb：留着它会让保存流程出现矛盾状态
   * （模式是自动、却带着一个手动数字），下次改成自定义还会突然跳回旧值。
   */
  const setMemoryMode = (next: 'auto' | 'custom' | undefined) => {
    if (next === 'custom') patch({ memoryMode: 'custom', memoryMb });
    else patch({ memoryMode: next, memoryMb: undefined });
  };

  const customized = countOverrides(overrides);
  const memoryMode = overrides.memoryMode;
  const memoryMb = overrides.memoryMb ?? global.memoryMb;
  const globalMemoryMb = global.memoryMode === 'auto' ? autoMb : global.memoryMb;
  const sliderValue = Math.min(maxIndex, Math.max(MIN_SLIDER_INDEX, gbToSliderIndex(memoryMb / 1024)));

  const windowCustom = overrides.windowWidth !== undefined && overrides.windowHeight !== undefined;
  const windowWidth = overrides.windowWidth ?? global.windowWidth;
  const windowHeight = overrides.windowHeight ?? global.windowHeight;

  const booleanRow = (
    label: string,
    hint: string,
    key: 'fullscreen' | 'showGameLog' | 'optimizeMemory',
    globalValue: boolean,
  ) => (
    <div className={styles.row} key={key}>
      <div className={styles.copy}>
        <strong>{label}</strong>
        <span>{hint}</span>
      </div>
      <div className={styles.control}>
        <TriStateSwitch
          value={overrides[key]}
          globalValue={globalValue}
          onChange={next => set(key, next)}
          label={label}
        />
      </div>
    </div>
  );

  return (
    <div className={styles.page}>
      <div className={styles.inner}>
        <header className={styles.header}>
          <div className={styles.headerCopy}>
            {/* 词典里的文案自带箭头，所以这里不再放图标，避免出现两个箭头。 */}
            <button type="button" className={styles.back} onClick={onBack}>
              {t('app.backToLibrary')}
            </button>
            <span className={styles.eyebrow}>{t('app.instanceManagement')}</span>
            <h1 className={styles.title}>{instance.name}</h1>
          </div>
          <div className={styles.headerActions}>
            <button type="button" className={`${styles.action} ${styles.launch}`} onClick={onLaunch}>
              <Play size={13} strokeWidth={2.2} /> {t('app.launch')}
            </button>
            <button type="button" className={styles.action} onClick={onOpenGlobal}>
              <Settings2 size={13} /> {t('override.openGlobal')}
            </button>
          </div>
        </header>

        <div className={styles.facts}>
          <span className={styles.fact}>
            {t('override.factVersion')} <b>{instance.version}</b>
          </span>
          <span className={styles.fact}>
            {t('override.factLoader')} <b>{instance.loader && instance.loader !== 'vanilla' ? instance.loader : t('app.vanilla')}</b>
          </span>
          <span className={styles.fact}>
            {t('override.factMods')} <b>{instance.modCount}</b>
          </span>
          <span className={styles.fact}>
            {customized > 0
              ? t('override.customized', { count: customized })
              : t('override.followAll')}
          </span>
        </div>

        <section className={styles.section}>
          <div className={styles.sectionHead}>
            <span className={styles.sectionIcon}><Rocket size={16} strokeWidth={1.8} /></span>
            <h2>{t('override.sectionBehavior')}</h2>
            <small>{t('override.followHint')}</small>
          </div>
          {booleanRow(t('override.fullscreen'), t('override.fullscreenHint'), 'fullscreen', global.fullscreen)}
          {booleanRow(t('override.showLog'), t('override.showLogHint'), 'showGameLog', global.showGameLog)}
          {booleanRow(t('override.optimize'), t('override.optimizeHint'), 'optimizeMemory', global.optimizeMemory)}
        </section>

        <section className={styles.section}>
          <div className={styles.sectionHead}>
            <span className={styles.sectionIcon}><MemoryStick size={16} strokeWidth={1.8} /></span>
            <h2>{t('override.sectionMemory')}</h2>
            <small>{t('override.memoryGlobal', { values: { size: formatGb(globalMemoryMb) } })}</small>
          </div>
          <div className={styles.block}>
            <div className={styles.row}>
              <div className={styles.copy}>
                <strong>{t('memory.mode')}</strong>
                <span>{t('override.memoryHint', { values: { size: formatGb(globalMemoryMb) } })}</span>
              </div>
              <div className={styles.control}>
                <SegmentedChoice
                  value={memoryMode}
                  onChange={setMemoryMode}
                  label={t('memory.mode')}
                  options={[
                    { value: 'auto' as const, text: t('memory.auto') },
                    { value: undefined, text: t('override.inherit'), neutral: true },
                    { value: 'custom' as const, text: t('memory.custom') },
                  ]}
                  caption={memoryMode === undefined
                    ? t('override.globalIs', { values: { state: global.memoryMode === 'auto' ? t('memory.auto') : t('memory.custom') } })
                    : undefined}
                />
              </div>
            </div>

            {/* 自动档：把将要分配的数字直接写出来，而不是留一条死滑条。 */}
            {(memoryMode === 'auto' || (memoryMode === undefined && global.memoryMode === 'auto')) && (
              <div className={styles.detail}>
                <div className={styles.detailHead}>
                  <output>{formatGb(memoryMode === undefined ? globalMemoryMb : autoMb)}</output>
                  <small>{t('override.memoryAutoNote', { count: instance.modCount })}</small>
                </div>
              </div>
            )}

            {(memoryMode === 'custom' || (memoryMode === undefined && global.memoryMode === 'custom')) && (
              <div className={styles.detail}>
                <div className={styles.detailHead}>
                  <output>{formatGb(memoryMode === undefined ? global.memoryMb : memoryMb)}</output>
                  <small>
                    {memoryMode === undefined ? t('override.memorySliderNote') : t('override.memorySliderOwn')}
                  </small>
                </div>
                <input
                  className={styles.range}
                  type="range"
                  min={MIN_SLIDER_INDEX}
                  max={maxIndex}
                  step={1}
                  value={sliderValue}
                  aria-label={t('memory.max')}
                  onChange={event => {
                    const gb = sliderIndexToGb(Number(event.target.value));
                    patch({ memoryMode: 'custom', memoryMb: Math.max(MIN_MEMORY_MB, Math.round(gb * 1024)) });
                  }}
                />
                <div className={styles.ticks}>
                  <span>{formatGb(MIN_MEMORY_MB)}</span>
                  <span>{formatGb(totalGb * 1024)}</span>
                </div>
              </div>
            )}
          </div>
        </section>

        <section className={styles.section}>
          <div className={styles.sectionHead}>
            <span className={styles.sectionIcon}><Monitor size={16} strokeWidth={1.8} /></span>
            <h2>{t('override.sectionWindow')}</h2>
            <small>{t('override.windowGlobal', { values: { width: global.windowWidth, height: global.windowHeight } })}</small>
          </div>
          <div className={styles.block}>
            <div className={styles.row}>
              <div className={styles.copy}>
                <strong>{t('override.windowSize')}</strong>
                <span>{t('override.windowHint', { values: { width: global.windowWidth, height: global.windowHeight } })}</span>
              </div>
              <div className={styles.control}>
                <SegmentedChoice
                  value={windowCustom}
                  onChange={custom => {
                    if (custom) {
                      onChange({ ...overrides, windowWidth: global.windowWidth, windowHeight: global.windowHeight });
                    } else {
                      onChange(clearOverride(clearOverride(overrides, 'windowWidth'), 'windowHeight'));
                    }
                  }}
                  label={t('override.windowSize')}
                  options={[
                    { value: false, text: t('override.inherit'), neutral: true },
                    { value: true, text: t('settings.windowCustom') },
                  ]}
                />
              </div>
            </div>
            {windowCustom && (
              <div className={styles.detail}>
                <div className={styles.dimensions}>
                  <input
                    type="number"
                    min="640"
                    max="7680"
                    value={windowWidth}
                    aria-label={t('override.widthAria')}
                    onChange={event => set('windowWidth', Number(event.target.value))}
                  />
                  <span>×</span>
                  <input
                    type="number"
                    min="360"
                    max="4320"
                    value={windowHeight}
                    aria-label={t('override.heightAria')}
                    onChange={event => set('windowHeight', Number(event.target.value))}
                  />
                  <span>{t('settings.pixels')}</span>
                </div>
              </div>
            )}
          </div>
        </section>

        <section className={styles.section}>
          <div className={styles.sectionHead}>
            <span className={styles.sectionIcon}><SlidersHorizontal size={16} strokeWidth={1.8} /></span>
            <h2>{t('override.sectionAdvanced')}</h2>
            <small>{t('override.advancedHint')}</small>
          </div>
          {(
            [
              { key: 'extraJvmArgs', label: t('settings.extraJvm'), hint: t('override.jvmHint'), placeholder: t('settings.jvmPlaceholder'), rows: 3 },
              { key: 'gameArgs', label: t('settings.gameArgs'), hint: t('override.gameArgsHint'), placeholder: t('settings.gameArgsPlaceholder'), rows: 2 },
              { key: 'preLaunchCommand', label: t('settings.preLaunch'), hint: t('override.preLaunchHint'), placeholder: t('settings.preLaunchPlaceholder'), rows: 2 },
            ] as const
          ).map(field => {
            const value = overrides[field.key];
            const custom = value !== undefined;
            return (
              <div className={styles.block} key={field.key}>
                <div className={styles.row}>
                  <div className={styles.copy}>
                    <strong>{field.label}</strong>
                    <span>{field.hint}</span>
                  </div>
                  <div className={styles.control}>
                    <SegmentedChoice
                      value={custom}
                      onChange={next => set(field.key, next ? '' : undefined)}
                      label={field.label}
                      options={[
                        { value: false, text: t('override.inherit'), neutral: true },
                        { value: true, text: t('memory.custom') },
                      ]}
                    />
                  </div>
                </div>
                {custom && (
                  <textarea
                    className={styles.text}
                    rows={field.rows}
                    value={value}
                    placeholder={field.placeholder}
                    aria-label={field.label}
                    onChange={event => set(field.key, event.target.value)}
                  />
                )}
              </div>
            );
          })}
        </section>

        <section className={styles.section}>
          <div className={styles.sectionHead}>
            <span className={styles.sectionIcon}><Coffee size={16} strokeWidth={1.8} /></span>
            <h2>{t('override.sectionJava')}</h2>
            <small>{t('override.javaGlobal', { values: { count: javaRuntimes.length } })}</small>
          </div>
          <div className={styles.javaCard}>
            <InstanceJavaCard
              instance={instance}
              runtimes={javaRuntimes}
              setting={javaSetting ?? { ...DEFAULT_INSTANCE_JAVA }}
              globalPath={globalJavaPath}
              onChange={onJavaSettingChange}
              onManageJava={onManageJava}
              embedded
            />
          </div>
        </section>

        <div className={styles.footer}>
          <span className={styles.footerText}>
            {customized > 0
              ? t('override.summary', { count: customized })
              : t('override.none')}
          </span>
          <button
            type="button"
            className={styles.reset}
            disabled={customized === 0}
            onClick={() => onChange({})}
          >
            <RotateCcw size={13} /> {t('override.resetAll')}
          </button>
        </div>
      </div>
    </div>
  );
}

export default InstanceSettings;
