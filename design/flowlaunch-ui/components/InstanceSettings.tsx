// 实例管理页。
//
// 这个页面回答的问题是「这个实例和全局有什么不一样」。所以每一项都先给出
// 全局当前值，再由用户决定是否覆盖——默认全部跟随全局，未覆盖时页面看起来
// 就是一张「与全局一致」的清单，而不是逼用户重新做一遍设置。
//
// 控件选择：
//   - 布尔项是「关 / 开」两档按钮，左边另放一个「跟随全局」徽标表示覆盖关系。
//     「跟随全局」不是取值，塞进按钮会变成语义不一致的中间档；而完全去掉继承
//     入口又会让用户分不清「这是全局的值」还是「我在这里设成了这个值」，所以
//     把继承做成按钮前可点击的徽标，亮起即表示未覆盖。
//   - 值项保留两档（跟随全局 / 自定义），只有选中自定义时才展开具体输入框。
//     展开而不是常驻，是为了让默认状态足够安静：一屏能看到所有可覆盖项，
//     不必滚过一堆灰掉的输入框。
//   - 所有控件都由 SegmentedChoice 渲染，保证同一列里的高度、圆角、字号一致。

import { useMemo, useState } from 'react';
import {
  Coffee,
  HardDrive,
  Cpu,
  MemoryStick,
  Monitor,
  Package,
  Play,
  Rocket,
  RotateCcw,
  Settings2,
  SlidersHorizontal,
  Trash2,
  Wrench,
  FolderTree,
} from 'lucide-react';
import type { GameInstance } from '../types';
import { useI18n } from '../utils/i18n';
import { errorText, resetInstanceSettings } from '../utils/native';
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
import { ModManager } from './ModManager';
import { LoaderInstallCard } from './LoaderInstallCard';
import { BackupDiagnostics } from './BackupDiagnostics';
import { InstanceMaintenance } from './InstanceMaintenance';
import { ExportPackCard, InstanceFiles } from './InstanceFiles';
import { OverrideChoice } from './OverrideChoice';
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
  /** 请求删除本实例。实际删除由父组件确认后执行。 */
  onDelete: () => void;
  /** 模组开关影响实例卡片上的计数，需要父组件刷新。 */
  onModsChanged: () => void;
  /** 模组页的提示与错误统一走父组件的提示条。 */
  onNotice: (message: string) => void;
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
  onDelete,
  onModsChanged,
  onNotice,
}: InstanceSettingsProps) {
  const { t } = useI18n();
  /** 重置进行中，用于禁用按钮避免重复提交。 */
  const [resetting, setResetting] = useState(false);

  /**
   * 重置全部覆盖。
   *
   * 必须调后端：只把本地状态清空的话，后端表里还留着旧的覆盖值，用户重启
   * 启动器后会看到「已恢复默认」的设置又变回自定义——而且没有任何提示说明
   * 哪一步没生效。清空本地状态放在后端成功之后，失败时界面不会假装已恢复。
   */
  const resetAll = async () => {
    setResetting(true);
    try {
      await resetInstanceSettings(instance.id);
      onChange({});
      onNotice(t('override.resetDone'));
    } catch (error) {
      onNotice(errorText(error, '无法恢复默认设置。'));
    } finally {
      setResetting(false);
    }
  };

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
            {/*
              删除放在这里而不是游戏库卡片上：卡片里三枚按钮会把实例名压到显示
              不全，而删除又不可撤销，值得让用户先进到详情页、再点一次确认。
              样式用独立的 danger 修饰类，和「启动」的强调色区分开。
            */}
            <button type="button" className={`${styles.action} ${styles.danger}`} onClick={onDelete}>
              <Trash2 size={13} strokeWidth={2} /> {t('instance.delete')}
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
                {/*
                  用 OverrideChoice 而不是三档 SegmentedChoice：三项里有一项是
                  「跟随全局」，它是覆盖关系而不是取值。混在三档里会让按钮组从
                  两档变成三档，宽度比同一列的其它行多出一段，整列就对不齐了。
                */}
                <OverrideChoice
                  inheriting={memoryMode === undefined}
                  value={memoryMode ?? global.memoryMode}
                  onChange={setMemoryMode}
                  onInherit={() => setMemoryMode(undefined)}
                  label={t('memory.mode')}
                  globalState={global.memoryMode === 'auto' ? t('memory.auto') : t('memory.custom')}
                  options={[
                    { value: 'auto' as const, text: t('memory.auto') },
                    { value: 'custom' as const, text: t('memory.custom') },
                  ]}
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

        {/*
          实例文件放在启动参数之后、加载器之前。

          为什么是这个位置：上面几块都是「配置」（改的是启动参数），下面几块是
          「内容」（模组、备份）。文件入口是内容管理的另一种方式——不想在启动器里
          逐个开关模组的人，会直接去文件夹操作。把它放在两者之间，符合「先设好
          怎么跑，再管里面有什么」的顺序。

          为什么集中成一块：存档、日志、配置文件的位置之前没有任何入口，用户要么
          自己翻安装目录，要么去搜。集中之后找东西只需要看这一列。
        */}
        <section className={styles.section}>
          <div className={styles.sectionHead}>
            <span className={styles.sectionIcon}><FolderTree size={16} strokeWidth={1.8} /></span>
            <h2>{t('files.sectionTitle')}</h2>
            <small>{t('files.sectionHint')}</small>
          </div>
          <div className={styles.javaCard}>
            <InstanceFiles instance={instance} onNotice={onNotice} />
            <ExportPackCard instance={instance} onNotice={onNotice} />
          </div>
        </section>

        {/*
          加载器安装排在模组之前：没装加载器时模组根本用不了，这是前置条件。
          已装加载器的实例这里只显示一行只读说明。
        */}
        <section className={styles.section}>
          <div className={styles.sectionHead}>
            <span className={styles.sectionIcon}><Cpu size={16} strokeWidth={1.8} /></span>
            <h2>{t('loader.sectionTitle')}</h2>
            <small>{t('loader.sectionHint')}</small>
          </div>
          <div className={styles.javaCard}>
            <LoaderInstallCard
              instance={instance}
              onNotice={onNotice}
              onInstalled={onModsChanged}
            />
          </div>
        </section>

        {/*
          模组管理放在实例设置页里，而不是单独的侧栏入口：模组是「这个实例的属性」，
          离开实例上下文去看模组列表没有意义。也正因为如此，它排在 Java 之后——
          先是可以覆盖的启动参数，再是实例内容。

          小标题里不写模组数量：`instance.modCount` 是实例列表加载时的快照，
          而页面里的统计是实时的，两者一旦不同就会在同一屏出现「3 / 3」和
          「2 / 3」两个矛盾的数字。数量统一由 ModManager 自己显示。
        */}
        <section className={styles.section}>
          <div className={styles.sectionHead}>
            <span className={styles.sectionIcon}><Package size={16} strokeWidth={1.8} /></span>
            <h2>{t('mods.sectionTitle')}</h2>
          </div>
          <div className={styles.javaCard}>
            <ModManager
              instance={instance}
              onNotice={onNotice}
              onChanged={onModsChanged}
            />
          </div>
        </section>

        {/*
          备份与诊断放在最后：它们不是日常操作，而是「出问题时的工具」。
          放在参数设置之后，日常用户不必先划过它。

          外层小标题用「数据」这个总称，而不是重复里面第一张卡片的
          「创建本地备份」——否则同一屏会出现两个一模一样的标题。
        */}
        <section className={styles.section}>
          <div className={styles.sectionHead}>
            <span className={styles.sectionIcon}><HardDrive size={16} strokeWidth={1.8} /></span>
            <h2>{t('backup.sectionTitle')}</h2>
          </div>
          <div className={styles.javaCard}>
            <BackupDiagnostics
              instance={instance}
              onNotice={onNotice}
              onRestored={onModsChanged}
            />
          </div>
        </section>

        {/*
          实例维护放在备份与诊断之前：检查客户端更新与导出整合包都属于「对整个
          实例做一次」的动作，比备份更常用，但也不属于日常开关。
        */}
        <section className={styles.section}>
          <div className={styles.sectionHead}>
            <span className={styles.sectionIcon}><Wrench size={16} strokeWidth={1.8} /></span>
            <h2>{t('maintain.sectionTitle')}</h2>
          </div>
          <div className={styles.javaCard}>
            <InstanceMaintenance
              instance={instance}
              onNotice={onNotice}
              onChanged={onModsChanged}
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
            disabled={customized === 0 || resetting}
            onClick={() => void resetAll()}
          >
            <RotateCcw size={13} /> {t('override.resetAll')}
          </button>
        </div>
      </div>
    </div>
  );
}

export default InstanceSettings;
