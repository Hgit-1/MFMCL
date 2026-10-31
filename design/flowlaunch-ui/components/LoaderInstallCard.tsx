// 实例的加载器安装（Fabric / Quilt / Forge / NeoForge）。
//
// 为什么需要这块界面：后端支持安装加载器，但界面里原本只有 Fabric/Quilt 且需要
// 手动点「读取版本」。结果是用户能创建原版实例，却很难把它变成模组实例。
//
// ## 两组加载器的差别（会让安装耗时差一个数量级）
//
// Fabric / Quilt 发布的是「profile json」，直接合并进版本配置，安装只需下载几十
// 个库，通常十几秒。
//
// Forge / NeoForge 发布的是「installer jar」，安装必须依次运行若干 Java processor
// （解压映射、下载 Mojang 映射、合并、切分客户端 jar、重命名、打二进制补丁）。
// 这个过程要下载上百 MB 的依赖并跑几分钟的 JVM。界面必须提前说明，否则用户会
// 以为卡死了。
//
// 三个必须如实呈现的约束：
//   1. 只有原版实例能安装加载器。后端会拒绝非原版实例（`每个实例仅允许安装一个
//      加载器`），所以已装加载器的实例直接显示为只读状态。
//   2. 加载器版本列表要按游戏版本查询。不同 Minecraft 版本对应的加载器版本
//      完全不同，不能给一份通用清单。
//   3. 安装是一个下载任务。它通过 `installation-task` 事件推送进度，装完要刷新
//      实例状态，否则卡片仍显示「未安装」。

import { useCallback, useEffect, useRef, useState } from 'react';
import { Cpu, Download } from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import {
  errorText,
  installLoader,
  loaderVersions,
  type LoaderKind,
  type LoaderVersion,
} from '../utils/native';
import type { GameInstance } from '../types';
import styles from './LoaderInstallCard.module.css';

interface LoaderInstallCardProps {
  instance: GameInstance;
  onNotice: (message: string) => void;
  /** 安装完成后刷新实例列表（状态与模组数都会变）。 */
  onInstalled: () => void;
}

/**
 * 可选加载器，按「安装代价」排序：先给快的两个。
 *
 * 顺序不是随意的：Fabric/Quilt 十几秒能装完，Forge/NeoForge 要跑几分钟的
 * processor。把便宜的先放，用户默认选择就是低成本的。
 */
const KINDS: { id: LoaderKind; label: string; heavy: boolean }[] = [
  { id: 'fabric', label: 'Fabric', heavy: false },
  { id: 'quilt', label: 'Quilt', heavy: false },
  { id: 'forge', label: 'Forge', heavy: true },
  { id: 'neoforge', label: 'NeoForge', heavy: true },
];

export function LoaderInstallCard({ instance, onNotice, onInstalled }: LoaderInstallCardProps) {
  const { t } = useI18n();
  const [kind, setKind] = useState<LoaderKind>('fabric');
  const [versions, setVersions] = useState<LoaderVersion[]>([]);
  const [state, setState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [loadError, setLoadError] = useState('');
  const [selected, setSelected] = useState('');
  const [installing, setInstalling] = useState(false);

  /** 已装加载器的实例不能再装。后端会拒绝，界面就不该提供这个入口。 */
  const hasLoader = instance.loader !== 'vanilla';
  const current = KINDS.find(item => item.id === kind) ?? KINDS[0];

  const load = useCallback(async () => {
    setState('loading');
    setLoadError('');
    try {
      const list = await loaderVersions(kind, instance.version);
      setVersions(list);
      // 默认选第一个稳定版；没有稳定版时退回列表首项，而不是留空让用户多点一次。
      const preferred = list.find(item => item.stable) ?? list[0];
      setSelected(preferred?.version ?? '');
      setState('ready');
    } catch (error) {
      setLoadError(errorText(error, '无法读取加载器版本。'));
      setState('error');
    }
  }, [instance.version, kind]);

  /*
   * 进入页面就自动检测可用版本。
   *
   * 「这个实例该装哪个加载器版本」是启动器完全有能力自己回答的问题（它知道实例
   * 的 MC 版本）。要求用户先点一次「读取版本」，只是把一个可自动的步骤变成了
   * 操作成本。
   *
   * 清空旧结果与发起请求放在同一个 effect 里：拆成两个的话，切换加载器时两个
   * effect 都会跑，顺序由声明顺序决定——若「清空」在「加载」之后，会把刚拿到的
   * 结果立刻清掉，界面表现为闪一下空白。
   */
  const lastQuery = useRef('');
  useEffect(() => {
    if (hasLoader) return;
    const key = `${instance.id}:${kind}:${instance.version}`;
    if (lastQuery.current === key) return;
    lastQuery.current = key;
    setVersions([]);
    setSelected('');
    void load();
  }, [hasLoader, instance.id, instance.version, kind, load]);

  const run = async () => {
    if (!selected) return;
    setInstalling(true);
    try {
      await installLoader(instance.id, kind, selected);
      onNotice(t('loader.installed', { values: { kind, version: selected } }));
      onInstalled();
    } catch (error) {
      onNotice(errorText(error, '安装加载器失败。'));
      onInstalled();
    } finally {
      setInstalling(false);
    }
  };

  if (hasLoader) {
    return (
      <div className={styles.done}>
        <Cpu size={15} />
        <span>
          {t('loader.alreadyInstalled', { values: { kind: instance.loader ?? '', version: instance.version } })}
        </span>
      </div>
    );
  }

  return (
    <div className={styles.card}>
      <div className={styles.row}>
        <div className={styles.kinds} role="radiogroup" aria-label={t('loader.kind')}>
          {KINDS.map(item => (
            <button
              key={item.id}
              type="button"
              role="radio"
              aria-checked={kind === item.id}
              className={`${styles.kind} ${kind === item.id ? styles.kindActive : ''}`}
              onClick={() => setKind(item.id)}
            >
              {item.label}
            </button>
          ))}
        </div>

        {/*
          自动检测中不给按钮：让用户看到一个「读取版本」的按钮在转圈，会让他以为
          这一步需要自己触发。仅在失败时给「重试」。
        */}
        {state === 'error' && (
          <Button size="sm" variant="ghost" onClick={() => void load()}>
            {t('common.retry')}
          </Button>
        )}
      </div>

      {/*
        Forge/NeoForge 的安装要跑一串 Java processor，比 Fabric/Quilt 慢一个
        数量级。提前说明，避免用户以为卡死。
      */}
      {current.heavy && (
        <p className={styles.kindNote}>{t('loader.heavyNote')}</p>
      )}

      {state === 'loading' && <p className={styles.hint}>{t('loader.detecting')}</p>}

      {state === 'error' && <p className={styles.error} role="alert">{loadError}</p>}

      {state === 'ready' && versions.length === 0 && (
        <p className={styles.hint}>{t('loader.noVersions', { values: { version: instance.version } })}</p>
      )}

      {state === 'ready' && versions.length > 0 && (
        <>
          <label className={styles.select}>
            <span>{t('loader.version')}</span>
            <select value={selected} onChange={event => setSelected(event.target.value)}>
              {versions.map(item => (
                <option key={item.version} value={item.version}>
                  {item.version}{item.stable ? '' : ` ${t('loader.beta')}`}
                </option>
              ))}
            </select>
          </label>
          <div className={styles.actions}>
            <Button
              icon={<Download size={13} />}
              loading={installing}
              disabled={!selected}
              onClick={() => void run()}
            >
              {t('loader.install')}
            </Button>
          </div>
          <p className={styles.hint}>{t('loader.installHint')}</p>
        </>
      )}
    </div>
  );
}
