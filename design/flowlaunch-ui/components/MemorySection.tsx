// 内存分配面板（PCL2 的「内存分配」卡片）。
//
// 交互参照 PCL2 的 PageSetupLaunch：
//   - 「自动配置 / 自定义」两个单选，自动模式下**滑条禁用并变灰**
//     （PCL: SliderRamCustom.IsEnabled = LaunchRamType = 1）。
//   - 自动模式给出的值不是写死的，而是按本机总内存 + 模组数量算出来的，
//     所以禁用状态下仍然显示它将要分配多少，避免用户只看到一条死滑条。
//   - 滑条最右端 = 设备总内存（maxSliderIndexForTotal），不是 16 GB 的死上限。
//   - 下面那条彩色内存条对应 PCL 的 PanRamDisplay：用分段宽度表示
//     「游戏分配 / 总内存」，右侧给出 GB 数值。

import React, { useEffect, useMemo, useState } from 'react';
import { AlertTriangle, Cpu, RotateCcw } from 'lucide-react';
import {
  DEVICE_MEMORY_OVERRIDE_KEY,
  MIN_MEMORY_MB,
  MIN_SLIDER_INDEX,
  autoAllocateMb,
  detectDeviceMemory,
  gbToSliderIndex,
  maxSliderIndexForTotal,
  saveDeviceMemoryOverride,
  sliderIndexToGb,
  type DeviceMemory,
} from '../utils/memory';
import { renderLocalized } from '../utils/i18n/localized';
import { useI18n } from '../utils/i18n';
import styles from './MemorySection.module.css';

interface MemorySectionProps {
  mode: 'auto' | 'custom';
  memoryMb: number;
  optimizeMemory: boolean;
  /** 当前实例的模组数量，用于自动配置的档位估算。 */
  modCount?: number;
  onModeChange: (mode: 'auto' | 'custom') => void;
  onMemoryChange: (memoryMb: number) => void;
  onOptimizeChange: (enabled: boolean) => void;
}

function formatGb(gb: number): string {
  return `${gb.toFixed(1)} GB`;
}

/** DeviceMemory.note 现在返回键，这里按当前语言渲染。 */
function deviceNote(language: Parameters<typeof renderLocalized>[0], device: DeviceMemory | null): string {
  return device ? renderLocalized(language, device.note) : '';
}

export function MemorySection({ mode, memoryMb, optimizeMemory, modCount = 0, onModeChange, onMemoryChange, onOptimizeChange }: MemorySectionProps) {
  const { t, language } = useI18n();
  const [device, setDevice] = useState<DeviceMemory | null>(null);
  const [editing, setEditing] = useState(false);
  const [draftGb, setDraftGb] = useState('');

  useEffect(() => {
    let alive = true;
    void detectDeviceMemory().then(result => { if (alive) setDevice(result); });
    return () => { alive = false; };
  }, []);

  const totalGb = (device?.totalMb ?? 8192) / 1024;
  const maxIndex = Math.max(MIN_SLIDER_INDEX + 1, maxSliderIndexForTotal(totalGb));
  const autoMb = useMemo(
    () => autoAllocateMb({ totalMb: device?.totalMb ?? 8192, modCount, usedMb: device?.usedMb }),
    [device?.totalMb, device?.usedMb, modCount],
  );
  const effectiveMb = mode === 'auto' ? autoMb : memoryMb;
  const gameGb = Math.min(effectiveMb / 1024, totalGb);
  /**
   * 系统已用内存。桌面版返回的是探测那一刻的快照；浏览器拿不到，保持 undefined。
   * 上限夹到总内存，防止后端/系统时序造成 used > total 的越界值。
   */
  const usedGb = device?.usedMb !== undefined
    ? Math.min(device.usedMb / 1024, totalGb)
    : undefined;
  // 自动模式下滑块也停在自动值的位置：禁用不代表指针应该留在别处，
  // 否则用户会误以为“自动”等于上次手动拖到的那个数。
  const sliderValue = Math.min(maxIndex, Math.max(MIN_SLIDER_INDEX, gbToSliderIndex(effectiveMb / 1024)));

  // 桌面版后端把「总内存 - max(1.5 GB, 20%)」作为可接受上限（见
  // crates/launcher-core/src/launch.rs 的 recommended_memory_for_total）。
  // 滑条按 PCL 允许拖到总内存，但超过这条线时提前说清楚。
  const backendCeilingGb = Math.max(0.5, totalGb - Math.max(1.5, totalGb * 0.2));
  const overBackendCeiling = mode === 'custom' && memoryMb / 1024 > backendCeilingGb + 0.05;

  const commitDraft = () => {
    const value = Number(draftGb);
    if (Number.isFinite(value) && value >= 1 && value <= 4096) {
      saveDeviceMemoryOverride(value * 1024);
      void detectDeviceMemory().then(setDevice);
    }
    setEditing(false);
  };

  return (
    <section className={styles.group}>
      <h3>{t('memory.title')}</h3>

      <div className={styles.modeRow}>
        <div className={styles.copy}>
          <strong>{t('memory.mode')}</strong>
          <span>{t('memory.modeHint')}</span>
        </div>
        {/*
          单个开关，两侧写着两个状态名，滑块停在哪边就是当前模式。
          之前是两个并排按钮，选中项用的是色调底（#EAF6FE），和轨道底色
          （#F3F4F6）几乎同色，所以看上去完全没有选中指示。
        */}
        <button
          type="button"
          role="switch"
          aria-checked={mode === 'custom'}
          aria-label={t('memory.modeAria', { values: { mode: t(mode === 'auto' ? 'memory.auto' : 'memory.custom') } })}
          className={styles.modeSwitch}
          onClick={() => onModeChange(mode === 'auto' ? 'custom' : 'auto')}
        >
          <span className={`${styles.modeLabel} ${mode === 'auto' ? styles.modeLabelActive : ''}`}>
            {t('memory.auto')}
          </span>
          <span className={`${styles.track} ${mode === 'custom' ? styles.trackOn : ''}`} aria-hidden="true">
            <i />
          </span>
          <span className={`${styles.modeLabel} ${mode === 'custom' ? styles.modeLabelActive : ''}`}>
            {t('memory.custom')}
          </span>
        </button>
      </div>

      <div className={`${styles.field} ${mode === 'auto' ? styles.fieldDisabled : ''}`} aria-disabled={mode === 'auto'}>
        <div className={styles.copy}>
          <strong>{t('memory.max')}</strong>
          <span>
            {mode === 'auto'
              ? t('memory.autoSummary', { count: modCount, values: { allocated: formatGb(autoMb / 1024), total: formatGb(totalGb), count: modCount } })
              : t('memory.maxHint')}
          </span>
        </div>
        <div className={styles.control}>
          <div className={styles.sliderHead}>
            <output className={styles.value}>{formatGb(gameGb)}</output>
            <span className={styles.total}>/ {formatGb(totalGb)}</span>
          </div>
          <input
            className={styles.slider}
            type="range"
            min={MIN_SLIDER_INDEX}
            max={maxIndex}
            step={1}
            value={sliderValue}
            disabled={mode === 'auto'}
            aria-label={t('memory.sliderAria')}
            aria-valuetext={formatGb(memoryMb / 1024)}
            style={{ '--fill': `${((sliderValue - MIN_SLIDER_INDEX) / Math.max(1, maxIndex - MIN_SLIDER_INDEX)) * 100}%` } as React.CSSProperties}
            onChange={event => {
              const gb = sliderIndexToGb(Number(event.target.value));
              onMemoryChange(Math.max(MIN_MEMORY_MB, Math.round(gb * 1024)));
            }}
          />
          <div className={styles.ticks} aria-hidden="true">
            <span>{formatGb(MIN_MEMORY_MB / 1024)}</span>
            <span>{formatGb(totalGb)}</span>
          </div>
        </div>
      </div>

      <div className={styles.barRow}>
        {/*
          三段式内存条：已使用（系统与其它进程）/ 游戏分配 / 剩余空闲。
          已使用只有桌面版能读到；浏览器里没有这个数据源，此时不画那一段，
          图例也不显示它——比起画一条假的 0，不给用户错误信息更好。
        */}
        <div
          className={styles.bar}
          role="img"
          aria-label={t('memory.barAria', {
            values: {
              used: usedGb === undefined ? '—' : formatGb(usedGb),
              game: formatGb(gameGb),
              total: formatGb(totalGb),
            },
          })}
        >
          {usedGb !== undefined && (
            <i className={styles.barUsed} style={{ width: `${(usedGb / Math.max(totalGb, 0.1)) * 100}%` }} />
          )}
          <i
            className={`${styles.barGame} ${usedGb === undefined ? styles.barGameFirst : ''}`}
            style={{ left: `${((usedGb ?? 0) / Math.max(totalGb, 0.1)) * 100}%`, width: `${(gameGb / Math.max(totalGb, 0.1)) * 100}%` }}
          />
        </div>
        <div className={styles.barLegend}>
          {usedGb !== undefined && (
            <span className={styles.legendUsed}>
              <i className={styles.dotUsed} />{t('memory.usedBySystem', { values: { size: formatGb(usedGb) } })}
            </span>
          )}
          <span><i className={styles.dotGame} />{t('memory.gameAllocated', { values: { size: formatGb(gameGb) } })}</span>
          <span className={styles.legendMuted}>
            {usedGb === undefined
              ? t('memory.total', { values: { size: formatGb(totalGb) } })
              : t('memory.free', { values: { size: formatGb(Math.max(0, totalGb - usedGb - gameGb)) } })}
          </span>
        </div>
      </div>

      <div className={styles.row}>
        <div className={styles.copy}>
          <strong>{t('memory.optimize')}</strong>
          <span>{t('memory.optimizeHint')}</span>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={optimizeMemory}
          aria-label={t('memory.optimize')}
          className={`${styles.switch} ${optimizeMemory ? styles.switchOn : ''}`}
          onClick={() => onOptimizeChange(!optimizeMemory)}
        ><i /></button>
      </div>

      {overBackendCeiling && (
        <p className={styles.warning} role="status">
          <AlertTriangle size={13} /> {t('memory.overCeiling', { values: { limit: formatGb(Math.floor(backendCeilingGb * 10) / 10) } })}
        </p>
      )}

      <div className={styles.deviceRow}>
        <Cpu size={13} />
        {editing ? (
          <>
            <label htmlFor="device-memory">{t('memory.deviceTotal')}</label>
            <input
              id="device-memory"
              type="number"
              min="1"
              max="4096"
              step="0.5"
              value={draftGb}
              autoFocus
              onChange={event => setDraftGb(event.target.value)}
              onKeyDown={event => { if (event.key === 'Enter') commitDraft(); if (event.key === 'Escape') setEditing(false); }}
              onBlur={commitDraft}
            />
            <span>GB</span>
          </>
        ) : (
          <>
            <span className={styles.deviceNote}>{device ? deviceNote(language, device) : t('memory.reading')}</span>
            <button type="button" className={styles.linkButton} onClick={() => { setDraftGb(totalGb.toFixed(1)); setEditing(true); }}>
              {device?.exact ? t('memory.reassign') : t('memory.correct')}
            </button>
            {device && device.source === 'manual' && (
              <button type="button" className={styles.linkButton} onClick={() => { localStorage.removeItem(DEVICE_MEMORY_OVERRIDE_KEY); void detectDeviceMemory().then(setDevice); }}>
                <RotateCcw size={11} />{t('memory.restoreAuto')}
              </button>
            )}
          </>
        )}
      </div>
    </section>
  );
}
