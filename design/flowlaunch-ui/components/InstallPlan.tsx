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
// ## 顺序
//
// 后端的状态机决定了顺序，界面必须遵守：
//   1. `create_instance`      —— 写一条记录，并把加载器请求写成标记文件
//   2. `install_instance`     —— 下载游戏文件；成功后消费标记并安装加载器
//
// 加载器请求必须在第 1 步就交给后端。之前这里只传名称和版本、把加载器留到第 3 步
// 单独调用，一旦那一步没有执行（面板被关掉、请求失败），实例就永久停在「名字写着
// Forge、记录却是 vanilla」的状态：界面上加载器显示原版，而用户以为已经装好了。

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { AlertTriangle, Boxes, Check, Cpu, Package } from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import { prefersReducedMotion } from '../utils/motion';
import {
  createInstance,
  errorText,
  installInstance,
  loaderVersions,
  type LoaderKind,
  type LoaderVersion,
} from '../utils/native';
import styles from './InstallPlan.module.css';

/** 可选加载器。`vanilla` 表示不装加载器。 */
type LoaderChoice = 'vanilla' | LoaderKind;

/**
 * 加载器的显示名。
 *
 * 单独一张表而不是内联三元：之前是 `choice === 'fabric' ? 'Fabric' : 'Quilt'`，
 * 加到四项就会退化成嵌套三元。加载器名是专有名词，三种语言都保持原文，不进词典。
 */
const LOADER_LABELS: Record<LoaderKind, string> = {
  fabric: 'Fabric',
  quilt: 'Quilt',
  forge: 'Forge',
  neoforge: 'NeoForge',
  legacyfabric: 'Legacy Fabric',
  liteloader: 'LiteLoader',
  cleanroom: 'Cleanroom',
  optifine: 'OptiFine',
};

export interface InstallPlanTarget {
  /** 游戏版本号，来自用户点的那一行。 */
  version: string;
}

interface InstallPlanProps {
  target: InstallPlanTarget | null;
  /** 关闭面板（放弃安装）。 */
  onClose: () => void;
  /** 刷新已写入的实例/任务，不关闭安装面板。 */
  onChanged: (instanceId: string) => Promise<void> | void;
  /** 仅全部步骤成功后关闭本次面板。 */
  onDone: (instanceId: string) => Promise<void> | void;
  onNotice: (message: string) => void;
}

type Phase = 'idle' | 'creating' | 'installing' | 'done' | 'failed';

type InstallProgress = {
  name: string;
  version: string;
  loader: LoaderChoice;
  loaderVersion: string;
  instanceId: string;
  vanillaDone: boolean;
};

/**
 * 找到最近的、真的能纵向滚动的祖先。
 *
 * 商店页的滚动发生在 `StoreWorkspace` 的 `.panel`（`overflow-y: auto`）里，而不是
 * 整个窗口。按 `window.innerHeight` 判断可见性会得出错误结论：元素可能远在窗口
 * 之内、却已经滚出那个容器的可视区。
 *
 * 判定要同时看 `overflow-y` 与「内容确实溢出」：`overflow: hidden` 的祖先（外层
 * 布局容器）也满足「不滚动」，但它是裁切而非滚动，把它当成目标会算出错误的位移。
 */
function scrollParentOf(node: HTMLElement | null): HTMLElement | null {
  let current = node?.parentElement ?? null;
  while (current) {
    const style = window.getComputedStyle(current);
    if (/(auto|scroll|overlay)/.test(style.overflowY) && current.scrollHeight > current.clientHeight) {
      return current;
    }
    current = current.parentElement;
  }
  return null;
}

export function InstallPlan({ target, onClose, onChanged, onDone, onNotice }: InstallPlanProps) {
  const { t } = useI18n();

  const [name, setName] = useState('');
  const [loader, setLoader] = useState<LoaderChoice>('vanilla');
  const [loaderVersion, setLoaderVersion] = useState('');
  const [versions, setVersions] = useState<LoaderVersion[]>([]);
  const [versionsState, setVersionsState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [versionsError, setVersionsError] = useState('');

  const [phase, setPhase] = useState<Phase>('idle');
  const [error, setError] = useState('');
  const progress = useRef<InstallProgress | null>(null);
  const session = useRef<object>({});
  const running = useRef<object | null>(null);
  const loaderRequest = useRef<object | null>(null);
  const [createdInstanceId, setCreatedInstanceId] = useState('');
  /**
   * 用户是否手动改过名字。
   *
   * 改过之后就不再自动覆盖：选加载器时把用户刚输入的名字冲掉，会让人觉得输入无效。
   * 没改过（默认名）时随加载器与版本自动更新，省掉一次输入。
   */
  const nameEdited = useRef(false);

  /** 面板自身与名称输入框，用于「点安装后把面板滚进视野」。见下面的 effect。 */
  const panelRef = useRef<HTMLDivElement>(null);
  const nameInputRef = useRef<HTMLInputElement>(null);

  /**
   * 默认实例名：`<游戏版本> <加载器> <加载器版本>`，原版只有版本号。
   *
   * 例如 `1.21.1 neoforge 21.1.265`、`1.20.1 forge 47.4.28`、`1.21.1`。
   * 这个名字同时会成为磁盘目录名（见后端 `directory_name_for`），所以它必须
   * 一眼能看出「哪个版本、什么加载器」——用户要手动改 jar 或拷存档时全靠它。
   */
  const defaultName = useCallback(
    (gameVersion: string, kind: LoaderChoice, version: string) => {
      if (kind === 'vanilla') return gameVersion;
      const label = LOADER_LABELS[kind];
      const suffix = version.trim();
      return suffix ? `${gameVersion} ${label.toLowerCase()} ${suffix}` : `${gameVersion} ${label.toLowerCase()}`;
    },
    [],
  );

  // 换目标版本时重置：版本变了，加载器列表与默认名都要跟着变。
  useLayoutEffect(() => {
    session.current = {};
    running.current = null;
    loaderRequest.current = null;
    progress.current = null;
    setCreatedInstanceId('');
    nameEdited.current = false;
    setName(target ? defaultName(target.version, 'vanilla', '') : '');
    setLoader('vanilla');
    setLoaderVersion('');
    setVersions([]);
    setVersionsState('idle');
    setVersionsError('');
    setPhase('idle');
    setError('');
    return () => { session.current = {}; loaderRequest.current = null; };
  }, [target, defaultName]);

  /**
   * 面板出现时把它滚进视野，并把焦点放到实例名输入框。
   *
   * ## 为什么必须有这一步
   *
   * 这个面板是**内联**的，渲染在版本列表上方。版本列表很长时，用户点的那一行
   * 「安装」按钮往往在屏幕下半部分，面板展开在列表顶部、完全在视野之外——点完
   * 看起来什么都没发生。用户会再点一次，或者以为按钮坏了。
   *
   * 之前把面板放在列表上方是为了「仍能看到自己点的是哪个版本」（见文件头注释），
   * 那个理由依然成立，但它只在面板可见时才成立。滚动过去正好补上这一环。
   *
   * ## 为什么不是无条件 `scrollIntoView`
   *
   * 面板已经完整可见时不该动：用户在列表中部点安装，界面突然跳一下，会以为
   * 自己点错了行。`block: 'nearest'` 又不够——面板在视野上方时它只把底部对齐到
   * 视口下沿，标题和「目标版本」会被推到屏幕外，用户看不到自己装的是哪个版本。
   * 所以这里先判断「是否已完整可见」，不可见时才按 `block: 'start'` 顶对齐滚动。
   *
   * 动效跟随用户的档位设置：关掉动效时用 `auto` 瞬移，不要忽略那个偏好。
   */
  useEffect(() => {
    if (!target) return;
    /*
     * 等一帧再算。
     *
     * 面板是在这次渲染中才挂载的，位置与高度要等布局完成才准确；立即调用会按
     * 「面板还不存在时」的尺寸判断，得出错误的结论。
     */
    const frame = requestAnimationFrame(() => {
      const panel = panelRef.current;
      if (panel) {
        /*
         * 用**滚动容器**的可视区判断，不是窗口。
         * 商店页的滚动发生在内部 `.panel` 上（见 `scrollParentOf` 的说明），用窗口
         * 高度判断会把「已经滚出容器」的元素当成可见，于是该滚的时候不滚。
         */
        const scroller = scrollParentOf(panel);
        const rect = panel.getBoundingClientRect();
        const top = scroller ? scroller.getBoundingClientRect().top : 0;
        const bottom = scroller
          ? top + scroller.clientHeight
          : (window.innerHeight || document.documentElement.clientHeight);
        // 留 8px 余量：正好贴着边缘时也滚一下，否则面板看起来像被切掉一块。
        const fullyVisible = rect.top >= top - 8 && rect.bottom <= bottom + 8;
        if (!fullyVisible) {
          panel.scrollIntoView({
            block: 'start',
            behavior: prefersReducedMotion() ? 'auto' : 'smooth',
          });
        }
      }
      /*
       * 焦点落在名称输入框：多数人打开这个面板就是要改名字。
       * `preventScroll` 防止焦点自身的滚动与上面那次滚动互相覆盖。
       */
      nameInputRef.current?.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(frame);
  }, [target]);

  /**
   * 选中加载器后拉取可用版本。
   *
   * 按「游戏版本 + 加载器」查，而不是拉全量再筛：加载器的版本号与游戏版本强绑定
   * （例如 Fabric Loader 0.16 只支持特定的 Minecraft 版本区间），服务端已经做了
   * 这层匹配，前端自己筛容易给出不适配的组合。
   */
  const loadVersions = useCallback(async (kind: LoaderKind, game: string) => {
    const request = {};
    loaderRequest.current = request;
    const owner = session.current;
    const fresh = () => loaderRequest.current === request && session.current === owner;
    setVersionsState('loading');
    setVersionsError('');
    setVersions([]);
    try {
      const list = await loaderVersions(kind, game);
      if (!fresh()) return;
      setVersions(list);
      // 默认选最新的稳定版：绝大多数用户要的就是这个。
      const preferred = list.find(item => item.stable)?.version ?? list[0]?.version ?? '';
      setLoaderVersion(preferred);
      /*
       * 加载器版本定下来之后更新名字：`1.21.1 neoforge` → `1.21.1 neoforge 21.1.265`。
       * 只有版本号才能说明「装的是哪个构建」，而它决定能装哪些模组。
       */
      if (!nameEdited.current && preferred) {
        setName(defaultName(game, kind, preferred));
      }
      setVersionsState('ready');
      if (list.length === 0) setVersionsError(t('install.noLoaderVersion'));
    } catch (cause) {
      if (!fresh()) return;
      setVersionsState('error');
      setVersionsError(errorText(cause, t('install.loaderListFailed')));
      setLoaderVersion('');
    }
  }, [t, defaultName]);

  const chooseLoader = (choice: LoaderChoice) => {
    if (running.current || progress.current?.instanceId) return;
    loaderRequest.current = null;
    setLoader(choice);
    setLoaderVersion('');
    setVersions([]);
    setVersionsState('idle');
    setVersionsError('');
    /*
     * 名字跟着加载器走，但只在用户没手动改过时。
     * 默认名（`1.21.1`）与选了加载器之后的（`1.21.1 neoforge ...`）是两种事实，
     * 停在旧名字会与实际装的加载器不符。
     */
    if (!nameEdited.current && target) {
      setName(defaultName(target.version, choice, ''));
    }
    if (choice !== 'vanilla' && target) void loadVersions(choice, target.version);
  };

  /** 加载器选好了才算可以开始；vanilla 直接可以。 */
  const loaderReady = loader === 'vanilla' || (versionsState === 'ready' && loaderVersion !== '');
  const busy = phase === 'creating' || phase === 'installing';

  const run = async () => {
    if (!target || running.current || !loaderReady) return;
    const trimmed = name.trim();
    if (!trimmed) { setError(t('install.nameRequired')); return; }
    const owner = session.current;
    const operation = {};
    running.current = operation;
    const fresh = () => session.current === owner;
    const plan: InstallProgress = progress.current?.instanceId ? progress.current : {
      name: trimmed, version: target.version, loader, loaderVersion,
      instanceId: '', vanillaDone: false,
    };
    progress.current = plan;
    setError('');
    let stage: Phase = 'creating';
    let succeeded = false;
    try {
      if (!plan.instanceId) {
        setPhase(stage);
        /*
         * 加载器一起交给创建请求。后端会把它写成标记文件，`install_instance`
         * 在官方文件补齐后自动消费并安装——同一条通路也是整合包导入用的那条。
         *
         * 不要改回「先建实例、稍后单独装加载器」：那一步只要没跑到，实例就会
         * 停在「名字带加载器、记录是原版」的状态，用户看不出少了什么。
         */
        const record = await createInstance(
          plan.name,
          plan.version,
          plan.loader === 'vanilla' ? null : `${plan.loader}@${plan.loaderVersion}`,
        );
        plan.instanceId = record.id;
        if (!fresh()) return;
        setCreatedInstanceId(record.id);
      }
      if (!plan.vanillaDone) {
        stage = 'installing';
        setPhase(stage);
        await installInstance(plan.instanceId);
        plan.vanillaDone = true;
        if (!fresh()) return;
      }
      succeeded = true;
      setPhase('done');
    } catch (cause) {
      if (!fresh()) return;
      setPhase('failed');
      const detail = errorText(cause, t('install.failed'));
      setError(stage === 'creating' ? detail : t('install.failedVanilla', { values: { detail } }));
    } finally {
      if (plan.instanceId) {
        try { await onChanged(plan.instanceId); }
        catch (cause) { if (fresh()) onNotice(errorText(cause)); }
      }
      if (running.current === operation) running.current = null;
    }
    if (!fresh() || !succeeded) return;
    onNotice(t('install.finished', { values: { name: plan.name } }));
    try { await onDone(plan.instanceId); }
    catch (cause) { if (fresh()) onNotice(errorText(cause)); }
  };

  if (!target) return null;

  const stepLabel = phase === 'creating' ? t('install.stepCreating')
    : phase === 'installing' ? t('install.stepInstalling')
      : phase === 'done' ? t('install.stepDone')
        : '';

  return (
    <div className={styles.panel} role="group" aria-label={t('install.title')} ref={panelRef}>
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
          ref={nameInputRef}
          value={name}
          onChange={event => {
            nameEdited.current = true;
            setName(event.target.value);
          }}
          placeholder={`Minecraft ${target.version}`}
          maxLength={48}
          disabled={busy || createdInstanceId !== ''}
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

            Forge / NeoForge 也在这里：后端早已支持（`LoaderKind` 含四个变体，
            安装走真实的 installer processor），之前只有本面板漏了它们，而下方还
            写着「暂不支持」——用户看到的是一个自相矛盾的界面。
            顺序按安装代价排列：Fabric/Quilt 十几秒，Forge/NeoForge 要跑几分钟的
            JVM processor。
          */}
          {(['vanilla', 'fabric', 'quilt', 'forge', 'neoforge', 'legacyfabric', 'liteloader', 'cleanroom', 'optifine'] as LoaderChoice[]).map(choice => (
            <button
              key={choice}
              type="button"
              role="radio"
              aria-checked={loader === choice}
              className={`${styles.loaderOption} ${loader === choice ? styles.loaderOptionActive : ''}`}
              onClick={() => chooseLoader(choice)}
              disabled={busy || createdInstanceId !== ''}
            >
              {choice === 'vanilla' ? <Package size={15} /> : <Boxes size={15} />}
              <span>{choice === 'vanilla' ? t('install.loaderNone') : LOADER_LABELS[choice]}</span>
              {loader === choice && <Check size={14} className={styles.loaderCheck} />}
            </button>
          ))}
        </div>
      </div>

      {/*
        Forge / NeoForge 的安装要跑一串 Java processor（解压映射、合并客户端 jar、
        打二进制补丁），比 Fabric/Quilt 慢一个数量级。提前说明，避免用户以为卡死。
      */}
      {(loader === 'forge' || loader === 'neoforge') && (
        <p className={styles.note}>{t('loader.heavyNote')}</p>
      )}

      {loader !== 'vanilla' && (
        <div className={styles.field}>
          <span>{t('install.loaderVersion')}</span>
          {versionsState === 'loading' && <p className={styles.hint}>{t('common.loading')}</p>}
          {versionsState === 'error' && <p className={styles.errorText}>{versionsError}</p>}
          {versionsState === 'ready' && versions.length > 0 && (
            <select
              value={loaderVersion}
              onChange={event => {
                setLoaderVersion(event.target.value);
                if (!nameEdited.current) setName(defaultName(target.version, loader, event.target.value));
              }}
              disabled={busy || createdInstanceId !== ''}
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

      {loader === 'optifine' && <div className={styles.optifine}>
        <Cpu size={15} />
        <div>
          <strong>{t('install.optifineTitle')}</strong>
          <span>{t('install.optifineNote')}</span>
        </div>
      </div>}

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
