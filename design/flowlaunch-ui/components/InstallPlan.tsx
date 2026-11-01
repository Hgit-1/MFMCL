// 原版版本安装面板。
//
// ## 这个面板解决的问题
//
// 原版列表里每一行本来只有一个「安装」按钮，点下去用的是**当前选中的实例**——
// 如果用户没选实例，或者选错了，就会装到别的地方；而他真正想做的往往是从零建一个
// 新实例。面板把「这次安装到底装成什么」在动手前说清楚：
//
//   实例名称 / 游戏版本 / 模组加载器（只能选一个）
//
// ## 为什么加载器是单选而不是多选
//
// Fabric / Quilt / Forge / NeoForge 是**互斥**的：一个实例只能有一个加载器，后端
// `install_loader` 也会在已有加载器时直接拒绝。做成多选就等于允许用户构造一个
// 必然失败的组合，然后让他在点完安装之后才看到错误。这里用单选，物理上排除这个
// 可能。
//
// OptiFine 有单独的说明：它不能与 Fabric 类加载器共存，且官方不提供可自动下载的
// 直链，所以这里只提供状态说明，不假装能一键安装。
//
// ## 顺序
//
// 后端的状态机决定了顺序，界面必须遵守：
//   1. `create_instance`      —— 只写一条记录，状态 not_installed
//   2. `install_instance`     —— 下载游戏文件，成功后状态 ready
//   3. `install_loader`       —— 要求状态已是 ready，且 loader 仍是 vanilla
//
// 所以「选 Fabric」不是把加载器参数塞进创建请求，而是在第 2 步完成之后再走第 3 步。

import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, Boxes, Check, Cpu, Package } from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import {
  createInstance,
  errorText,
  installInstance,
  installLoader,
  loaderVersions,
  type LoaderKind,
  type LoaderVersion,
} from '../utils/native';
import styles from './InstallPlan.module.css';

/** 可选加载器。`vanilla` 表示不装加载器。 */
type LoaderChoice = 'vanilla' | LoaderKind;

export interface InstallPlanTarget {
  /** 游戏版本号，来自用户点的那一行。 */
  version: string;
}

interface InstallPlanProps {
  target: InstallPlanTarget | null;
  /** 关闭面板（放弃安装）。 */
  onClose: () => void;
  /** 全部步骤完成：父组件刷新实例列表并选中新实例。 */
  onDone: (instanceId: string) => Promise<void> | void;
  onNotice: (message: string) => void;
}

type Phase = 'idle' | 'creating' | 'installing' | 'loader' | 'done' | 'failed';

export function InstallPlan({ target, onClose, onDone, onNotice }: InstallPlanProps) {
  const { t } = useI18n();

  const [name, setName] = useState('');
  const [loader, setLoader] = useState<LoaderChoice>('vanilla');
  const [loaderVersion, setLoaderVersion] = useState('');
  const [versions, setVersions] = useState<LoaderVersion[]>([]);
  const [versionsState, setVersionsState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [versionsError, setVersionsError] = useState('');

  const [phase, setPhase] = useState<Phase>('idle');
  const [error, setError] = useState('');

  // 换目标版本时重置：版本变了，加载器列表与默认名都要跟着变。
  useEffect(() => {
    if (!target) return;
    setName(`Minecraft ${target.version}`);
    setLoader('vanilla');
    setLoaderVersion('');
    setVersions([]);
    setVersionsState('idle');
    setVersionsError('');
    setPhase('idle');
    setError('');
  }, [target]);

  /**
   * 选中加载器后拉取可用版本。
   *
   * 按「游戏版本 + 加载器」查，而不是拉全量再筛：加载器的版本号与游戏版本强绑定
   * （例如 Fabric Loader 0.16 只支持特定的 Minecraft 版本区间），服务端已经做了
   * 这层匹配，前端自己筛容易给出不适配的组合。
   */
  const loadVersions = useCallback(async (kind: LoaderKind, game: string) => {
    setVersionsState('loading');
    setVersionsError('');
    setVersions([]);
    try {
      const list = await loaderVersions(kind, game);
      setVersions(list);
      // 默认选最新的稳定版：绝大多数用户要的就是这个。
      setLoaderVersion(list.find(item => item.stable)?.version ?? list[0]?.version ?? '');
      setVersionsState('ready');
      if (list.length === 0) setVersionsError(t('install.noLoaderVersion'));
    } catch (cause) {
      setVersionsState('error');
      setVersionsError(errorText(cause, t('install.loaderListFailed')));
      setLoaderVersion('');
    }
  }, [t]);

  const chooseLoader = (choice: LoaderChoice) => {
    setLoader(choice);
    setLoaderVersion('');
    setVersions([]);
    setVersionsState('idle');
    setVersionsError('');
    if (choice !== 'vanilla' && target) void loadVersions(choice, target.version);
  };

  /** 加载器选好了才算可以开始；vanilla 直接可以。 */
  const loaderReady = loader === 'vanilla' || (versionsState === 'ready' && loaderVersion !== '');
  const busy = phase === 'creating' || phase === 'installing' || phase === 'loader';

  const run = async () => {
    if (!target) return;
    const trimmed = name.trim();
    if (!trimmed) { setError(t('install.nameRequired')); return; }
    setError('');

    let instanceId = '';
    try {
      // 第 1 步：创建记录。
      setPhase('creating');
      const record = await createInstance(trimmed, target.version);
      instanceId = record.id;

      // 第 2 步：下载游戏文件。成功后后端会把状态改成 ready。
      setPhase('installing');
      await installInstance(instanceId);

      // 第 3 步：仅在用户选了加载器时执行。
      // 顺序不能提前：`install_loader` 要求实例已 ready，否则直接拒绝。
      if (loader !== 'vanilla') {
        setPhase('loader');
        await installLoader(instanceId, loader, loaderVersion);
      }

      setPhase('done');
      onNotice(t('install.finished', { values: { name: trimmed } }));
      await onDone(instanceId);
    } catch (cause) {
      setPhase('failed');
      /*
       * 失败时如实说明「已经做到哪一步」。
       *
       * 这一步很关键：如果游戏文件已经装完、只是加载器失败，那实例其实是可用的
       * （原版）。笼统地报「安装失败」会让用户以为整个实例不可用，甚至去删掉一个
       * 本来能用的实例。
       */
      const detail = errorText(cause, t('install.failed'));
      setError(
        phase === 'creating' ? detail
          : phase === 'installing' ? t('install.failedVanilla', { values: { detail } })
            : t('install.failedLoader', { values: { detail } }),
      );
      // 原版已经装好就刷新一次，让实例出现在列表里。
      if (instanceId) await Promise.resolve(onDone(instanceId)).catch(() => undefined);
    }
  };

  if (!target) return null;

  const stepLabel = phase === 'creating' ? t('install.stepCreating')
    : phase === 'installing' ? t('install.stepInstalling')
      : phase === 'loader' ? t('install.stepLoader')
        : phase === 'done' ? t('install.stepDone')
          : '';

  return (
    <div className={styles.panel} role="group" aria-label={t('install.title')}>
      <div className={styles.head}>
        <div>
          <h2>{t('install.title')}</h2>
          <p className={styles.subtitle}>{t('install.subtitle', { values: { version: target.version } })}</p>
        </div>
        <button type="button" className={styles.close} onClick={onClose} disabled={busy} aria-label={t('common.close')}>
          ×
        </button>
      </div>

      <label className={styles.field}>
        <span>{t('install.name')}</span>
        <input
          value={name}
          onChange={event => setName(event.target.value)}
          placeholder={`Minecraft ${target.version}`}
          maxLength={48}
          disabled={busy}
        />
        <small>{t('install.nameHint')}</small>
      </label>

      <div className={styles.field}>
        <span>{t('install.loader')}</span>
        <div className={styles.loaderGrid} role="radiogroup" aria-label={t('install.loader')}>
          {/*
            单选，且 vanilla 与各加载器互斥。
            这是「不会一口气装两个加载器」的第一道保证：界面上根本无法同时选中两个。
            第二道是后端的 `install_loader`，它会在实例已有加载器时直接拒绝。
          */}
          {(['vanilla', 'fabric', 'quilt'] as LoaderChoice[]).map(choice => (
            <button
              key={choice}
              type="button"
              role="radio"
              aria-checked={loader === choice}
              className={`${styles.loaderOption} ${loader === choice ? styles.loaderOptionActive : ''}`}
              onClick={() => chooseLoader(choice)}
              disabled={busy}
            >
              {choice === 'vanilla' ? <Package size={15} /> : <Boxes size={15} />}
              <span>{choice === 'vanilla' ? t('install.loaderNone') : choice === 'fabric' ? 'Fabric' : 'Quilt'}</span>
              {loader === choice && <Check size={14} className={styles.loaderCheck} />}
            </button>
          ))}
        </div>
      </div>

      {/*
        Forge 与 NeoForge 不在可选项里。
        后端 `LoaderKind` 只有 Fabric 与 Quilt；放一个选不了的按钮比不放更糟。
        这里用一行说明解释「为什么没有」，而不是让用户去猜。
      */}
      <p className={styles.note}>{t('install.unsupportedLoaders')}</p>

      {loader !== 'vanilla' && (
        <div className={styles.field}>
          <span>{t('install.loaderVersion')}</span>
          {versionsState === 'loading' && <p className={styles.hint}>{t('common.loading')}</p>}
          {versionsState === 'error' && <p className={styles.errorText}>{versionsError}</p>}
          {versionsState === 'ready' && versions.length > 0 && (
            <select
              value={loaderVersion}
              onChange={event => setLoaderVersion(event.target.value)}
              disabled={busy}
            >
              {versions.map(item => (
                <option key={item.version} value={item.version}>
                  {item.version}{item.stable ? ` (${t('install.stable')})` : ''}
                </option>
              ))}
            </select>
          )}
          {versionsState === 'ready' && versions.length === 0 && (
            <p className={styles.errorText}>{t('install.noLoaderVersion')}</p>
          )}
        </div>
      )}

      {/*
        OptiFine 只做说明，不给按钮。
        原因有两条，都不是能靠写代码绕过的：官方下载页要求经过广告页与定时跳转，
        没有可直接请求的直链；它也不能与 Fabric/Quilt 共存。给一个装不上的按钮
        只会浪费用户时间。
      */}
      <div className={styles.optifine}>
        <Cpu size={15} />
        <div>
          <strong>{t('install.optifineTitle')}</strong>
          <span>{t('install.optifineNote')}</span>
        </div>
      </div>

      {error && (
        <div className={styles.errorBox} role="alert">
          <AlertTriangle size={15} />
          <span>{error}</span>
        </div>
      )}

      <div className={styles.actions}>
        <Button variant="ghost" onClick={onClose} disabled={busy}>{t('common.cancel')}</Button>
        <Button onClick={() => void run()} loading={busy} disabled={!loaderReady || !name.trim()}>
          {busy ? t('install.working') : t('install.start')}
        </Button>
      </div>

      {/*
        说明按钮为什么不能点。
        只把按钮变淡的话，用户不知道是缺名称还是加载器版本还没选；这里明确指出来，
        而不是让他自己猜。
      */}
      {!busy && !loaderReady && (
        <p className={styles.step} role="status">{t('install.waitLoaderVersion')}</p>
      )}

      {stepLabel && (
        <p className={styles.step} role="status">{stepLabel}</p>
      )}
    </div>
  );
}
