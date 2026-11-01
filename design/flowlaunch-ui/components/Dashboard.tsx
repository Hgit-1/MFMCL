// 主页：一个居中的实例展示 + 右下角的大启动按钮。
//
// ## 为什么单独开一页而不是直接用游戏库
//
// 参考 PCL2 的启动页：它把「启动」做成整个界面上最显眼的一件事——一个大按钮，
// 下面一行小字写着将要启动的版本。用户打开启动器 90% 的意图就是「进游戏」，让
// 他先在一堆实例卡片里找、再点其中一个的「启动」，是把主路径藏进了次要界面。
//
// 游戏库仍然保留：那里是管理实例（新建、删除、修复、看模组数）的地方，同时也是
// 「主页按钮启动谁」的来源。两个页面的分工是「启动」与「管理」，而不是同一件事
// 的两种看法。
//
// ## 为什么按钮在右下角
//
// 主页只回答一个问题：「现在按下去会启动什么」。把实例信息放在正中、按钮贴在
// 页面右下角，视线落点固定，不随实例数量或名称长度漂移；右下角也是桌面端应用
// 里「主操作」的惯常位置（对话框的确定、PCL2 启动按钮所在的底部操作区）。
//
// ## 默认启动实例
//
// 与「游戏库里点哪个就启动哪个」并不冲突：主页那个按钮启动的是**默认实例**，
// 它可以在主页直接切换，也可以在游戏库的卡片上设成默认。默认值存在本地偏好里，
// 而不是复用后端的 `last_launched_at`——后者每次启动都会变，用户没法「固定」
// 一个想常玩的实例。

import { useEffect, useMemo, useRef } from 'react';
import { Play } from 'lucide-react';
import { Button } from './Button';
import { LoaderIcon } from './LoaderIcon';
import { BlockIcon } from './BlockIcon';
import { VANILLA_BLOCK } from '../utils/store/blocks';
import { useI18n } from '../utils/i18n';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import type { LoaderIcon as LoaderIconSpec } from '../utils/store/loaders';
import type { Account, GameInstance } from '../types';
import styles from './Dashboard.module.css';

interface DashboardProps {
  instances: GameInstance[];
  /** 主页按钮将要启动的实例；没有可用实例时为 undefined。 */
  target?: GameInstance;
  currentAccount?: Account;
  /** 正在启动的实例 id 与阶段（来自 `launch-progress`）。 */
  launching?: { instanceId: string; stage: string; progress: number } | null;
  /** 该实例是否已经在运行（此时按钮改为「关闭游戏」）。 */
  running: boolean;
  onLaunch: (instance: GameInstance) => void;
  onStop: (instanceId: string) => void;
  /** 切换默认启动实例。 */
  onSelectTarget: (instanceId: string) => void;
  /** 没有实例时引导去游戏库。 */
  onCreate: () => void;
  onOpenLibrary: () => void;
  /** 实例加载器对应的图标配置。 */
  iconForLoader: (loader: string | undefined) => LoaderIconSpec;
}

/** 启动阶段的展示文案。与 App 里的进度条共用同一套键。 */
const STAGE_LABELS: Record<string, MessageKey> = {
  preparing: 'app.launchStagePreparing',
  authenticating: 'app.launchStageAuthenticating',
  building: 'app.launchStageBuilding',
  starting: 'app.launchStageStarting',
};

export function Dashboard({
  instances,
  target,
  currentAccount,
  launching,
  running,
  onLaunch,
  onStop,
  onSelectTarget,
  onCreate,
  onOpenLibrary,
  iconForLoader,
}: DashboardProps) {
  const { t } = useI18n();

  /*
   * 把底部操作区的高度写进 CSS 变量 `--dashboard-dock-height`。
   *
   * 提示条固定贴在窗口右下角，正好会盖住这一页的启动按钮。它需要知道操作区有
   * 多高才能抬高，而那个高度随窗口宽度变化（窄窗口会折成两行）。
   *
   * 用 ResizeObserver 而不是 `window.resize` 监听：操作区高度还取决于下拉里的
   * 选项文字是否换行，这些都不触发布局尺寸之外的信号。变量写在 documentElement
   * 上，App 里的提示条在另一棵子树中也能读到。
   */
  const dockRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const node = dockRef.current;
    if (!node) return;
    const apply = (height: number) => {
      document.documentElement.style.setProperty('--dashboard-dock-height', `${Math.round(height)}px`);
    };
    apply(node.getBoundingClientRect().height);
    const observer = new ResizeObserver(entries => {
      const entry = entries[0];
      if (!entry) return;
      /*
       * 取 border-box 而不是 contentRect：操作区有上下内边距与一条上边框，
       * 提示条要避开的是包含内边距的整个可见高度。contentRect 只算内容区，
       * 量出来会比实际矮二十多像素，提示条仍然压住按钮的下沿。
       */
      const borderBox = entry.borderBoxSize?.[0];
      apply(borderBox ? borderBox.blockSize : node.getBoundingClientRect().height);
    });
    observer.observe(node);
    return () => {
      observer.disconnect();
      document.documentElement.style.removeProperty('--dashboard-dock-height');
    };
  }, [target]);

  /*
   * 默认实例下拉的排序：可启动的（ready）在前，其余的照样列出但禁用。
   *
   * 不直接把不可用的实例过滤掉：用户点开下拉发现少了一个实例，会以为是启动器
   * 丢了数据。列出来但标明状态（未安装 / 安装失败），他才知道该去修还是该新建。
   */
  const options = useMemo(() => {
    const rank = (instance: GameInstance) =>
      instance.status === 'ready' ? 0 : instance.status === 'error' ? 1 : 2;
    return [...instances].sort((a, b) => rank(a) - rank(b));
  }, [instances]);

  if (!target) {
    return (
      <div className={styles.page}>
        <div className={styles.stage}>
          <div className={styles.emptyState}>
            <h1>{t('dashboard.emptyTitle')}</h1>
            <p>{t('dashboard.emptyHint')}</p>
            <div className={styles.emptyActions}>
              <Button icon={<Play size={16} />} onClick={onCreate}>{t('app.createGame')}</Button>
              <Button variant="ghost" onClick={onOpenLibrary}>{t('dashboard.openLibrary')}</Button>
            </div>
          </div>
        </div>
      </div>
    );
  }

  const canLaunch = target.status === 'ready';
  const isLaunching = launching?.instanceId === target.id;
  const percent = isLaunching ? Math.round(launching.progress * 100) : 0;

  return (
    <div className={styles.page}>
      {/*
        实例信息区。PCL2 把版本名写在按钮下方；这里反过来把「要启动的实例」放在
        页面正中：按钮是动作，实例是宾语，读起来是
        「启动 —— Fabric 优化整合」，比反过来更自然。
      */}
      <div className={styles.stage}>
        <div className={styles.hero}>
          <div className={styles.heroIcon}>
            <LoaderIcon
              {...iconForLoader(target.loader)}
              size={72}
              muted={target.status === 'error'}
              fallback={<BlockIcon {...VANILLA_BLOCK} size={72} />}
            />
          </div>
          <h1 className={styles.heroName}>{target.name}</h1>
          <div className={styles.heroMeta}>
            <span>Minecraft {target.version}</span>
            {target.loader !== 'vanilla' && <span>{target.loader}</span>}
            {target.modCount > 0 && (
              <span>{t('app.modCount', { count: target.modCount })}</span>
            )}
          </div>
          {currentAccount && (
            <div className={styles.heroAccount}>
              {t('dashboard.account', { values: { name: currentAccount.username } })}
            </div>
          )}
          {/*
            未就绪时在信息区就说明原因，而不是等用户去点一个灰按钮。
            按钮本身会被禁用（见下方），两处配合：先解释，再阻止。
          */}
          {!canLaunch && (
            <p className={styles.blockedHint}>
              {target.status === 'error' ? t('dashboard.needRepair') : t('dashboard.needInstall')}
            </p>
          )}
        </div>
      </div>

      {/*
        启动进度。放在操作区正上方：用户点了右下角的按钮，视线就在按钮附近，
        把阶段与百分比放在这里比另起一个浮层或屏幕底部的细条更容易被看到。
        App 底部那条全局进度条仍然保留，用于「已经切到别的页面」的情况。
      */}
      {isLaunching && (
        <div className={styles.progressRow} role="status" aria-live="polite">
          <div className={styles.progressTrack}>
            <div className={styles.progressFill} style={{ width: `${percent}%` }} />
          </div>
          <span className={styles.progressStage}>
            {t(STAGE_LABELS[launching.stage] ?? 'app.launchStagePreparing')}
          </span>
        </div>
      )}

      {/*
        底部操作区：左侧选「启动什么」，右侧是按钮。
        下拉放在这里而不是设置页：它是这个按钮的直接宾语，与按钮同处一行才不会被
        读成全局设置；游戏库里同样可以改（卡片上的图钉），两处写同一个值。
        不在这里重复实例图标与名称：上方信息区已经写全了，同一屏出现两次反而
        让「哪个才是当前实例」变得含糊。
      */}
      <div className={styles.dock} ref={dockRef}>
        <div className={styles.dockPicker}>
          <label htmlFor="dashboard-target">{t('dashboard.defaultTarget')}</label>
          <select
            id="dashboard-target"
            value={target.id}
            onChange={event => onSelectTarget(event.target.value)}
          >
            {options.map(instance => (
              <option key={instance.id} value={instance.id} disabled={instance.status !== 'ready'}>
                {instance.name} · {instance.version}
                {instance.status === 'error'
                  ? ` · ${t('dashboard.stateBroken')}`
                  : instance.status !== 'ready'
                    ? ` · ${t('dashboard.stateNotInstalled')}`
                    : ''}
              </option>
            ))}
          </select>
        </div>

        <div className={styles.dockActions}>
          <Button variant="ghost" onClick={onOpenLibrary}>
            {t('dashboard.manage')}
          </Button>
          {/*
            主按钮。三种状态对应三件事：
              ready + 未运行   → 启动
              ready + 运行中   → 关闭游戏（避免重复启动直接报错）
              未就绪           → 禁用并说明原因
          */}
          {running ? (
            <Button
              size="lg"
              variant="danger"
              className={styles.launchButton}
              onClick={() => onStop(target.id)}
            >
              {t('app.stopGame')}
            </Button>
          ) : (
            <Button
              size="lg"
              className={styles.launchButton}
              icon={isLaunching ? undefined : <Play size={20} />}
              loading={isLaunching}
              disabled={!canLaunch}
              onClick={() => onLaunch(target)}
            >
              {isLaunching ? t('dashboard.launching') : t('app.launch')}
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
