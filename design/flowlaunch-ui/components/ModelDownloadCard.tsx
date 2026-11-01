// 设置里的「本地 AI 模型」卡片。
//
// ## 它解决的问题
//
// 崩溃分析有两半：规则（即时、离线、不需要任何额外文件）和本地模型（需要用户
// 另下一个 1.1 GB 的模型文件，并自己起推理引擎）。不下载模型时功能其实**已经
// 可用**，只是少一段解释。但此前卸载/未配置的环境里没有任何地方说明这一点，
// 用户只会看到「模型未连接」。
//
// 所以这张卡片做三件事：
//   1. 直说规则分析不需要它（避免用户以为功能坏了）；
//   2. 给出下载按钮，并从最快的源下载；
//   3. 如实说明「下载完还需要推理引擎」——不把「下载完成」伪装成「功能已就绪」。
//
// ## 为什么先测速再下载
//
// 模型托管在 HuggingFace、hf-mirror、ModelScope 三处，同一台机器上实测吞吐能差
// 四倍，而且哪个快取决于用户所在网络。写死一个源等于让一半用户用慢的。测速结果
// 也直接显示出来：全部不可用时用户能看到具体原因，而不是对着失败的按钮猜。
//
// ## 布局
//
// 状态行（已下载 / 未下载 / 不完整）→ 源列表 → 操作按钮。源列表只在「未下载」或
// 「正在下载」时展开：已下载的用户不需要看它，常驻只会让卡片变长。

import { useCallback, useEffect, useRef, useState } from 'react';
import { AlertTriangle, CheckCircle2, Cpu, Download, HardDriveDownload, Trash2 } from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import {
  cancelModelDownload,
  downloadModel,
  errorText,
  formatBytes,
  getCrashAiStatus,
  getModelStatus,
  isDesktopShell,
  pickFile,
  stopCrashAiEngine,
  onModelFailed,
  onModelProgress,
  probeModelSources,
  removeModel,
  type ModelProgress,
  type ModelSourceStatus,
  type ModelStatus,
  type CrashAiStatus,
} from '../utils/native';
import styles from './ModelDownloadCard.module.css';

interface ModelDownloadCardProps {
  /** 用户指定的 llama-server 路径；空表示自动查找。 */
  serverPath?: string
  onChangeServerPath?: (path: string) => void
}

export function ModelDownloadCard({ serverPath = '', onChangeServerPath }: ModelDownloadCardProps) {
  const { t } = useI18n();
  const [status, setStatus] = useState<ModelStatus | null>(null);
  /**
   * 推理引擎的状态。
   *
   * 与「模型是否下载」分开：下载完模型不等于能分析，引擎缺失时两者都齐了也用不了。
   * 界面上必须把这两件事分别说清，否则用户会反复重下模型。
   */
  const [engine, setEngine] = useState<CrashAiStatus | null>(null);
  const [sources, setSources] = useState<ModelSourceStatus[]>([]);
  const [progress, setProgress] = useState<ModelProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  /** 中性提示（成功、已取消）。与 `error` 分开，避免把「取消」标成故障。 */
  const [notice, setNotice] = useState('');
  /** 测速/下载都在进行时用它做进度条的动画开关。 */
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    void getModelStatus()
      .then(next => { if (mounted.current) setStatus(next); })
      .catch(() => { /* 状态读不到时按未下载处理，不阻塞设置面板 */ });
    void getCrashAiStatus()
      .then(next => { if (mounted.current) setEngine(next); })
      .catch(() => { /* 引擎状态读不到时不显示那一行，而不是显示错误 */ });
    return () => { mounted.current = false; };
  }, []);

  /*
   * 订阅下载进度。
   *
   * 进度事件只在桌面版有，预览模式返回 null，这里不用区分——没有事件就没有进度，
   * 界面停在「未下载」状态，正好是预览里要调整的那一屏。
   */
  useEffect(() => {
    let disposed = false;
    const cleanups: Array<() => void> = [];
    void onModelProgress(payload => {
      if (disposed) return;
      setProgress(payload);
      if (payload.stage === 'completed') {
        // 完成后回读一次：`installed` 是后端算的，照着它显示比前端自己推断可靠。
        void getModelStatus().then(next => { if (!disposed) setStatus(next); }).catch(() => undefined);
      }
    }).then(unlisten => { if (unlisten) cleanups.push(unlisten); else undefined; });
    void onModelFailed(message => {
      if (disposed) return;
      setError(message);
    }).then(unlisten => { if (unlisten) cleanups.push(unlisten); });
    return () => {
      disposed = true;
      cleanups.forEach(off => off());
    };
  }, []);

  const refresh = useCallback(async () => {
    try {
      setStatus(await getModelStatus());
    } catch (cause) {
      setError(errorText(cause, '无法读取模型状态。'));
    }
  }, []);

  const probe = async () => {
    setError('');
    try {
      setSources(await probeModelSources());
    } catch (cause) {
      setError(errorText(cause, '无法测速。'));
    }
  };

  const startDownload = async () => {
    setBusy(true);
    setError('');
    setNotice('');
    setProgress({ stage: 'probing', received: 0, total: status?.expectedBytes ?? 0, source: null, bytesPerSecond: null });
    try {
      const next = await downloadModel();
      setStatus(next);
      // 下载成功后把源列表收起来：它已经完成使命。
      setSources([]);
      setNotice(t('model.downloaded', { values: { size: formatBytes(next.expectedBytes) } }));
    } catch (cause) {
      const message = errorText(cause, '下载模型失败。');
      /*
       * 用户主动取消不是错误。
       *
       * 后端对取消返回的是「已取消下载」这句话。把它当成一条红色错误提示会让人
       * 以为自己弄坏了什么，而实际上什么都没发生。区分开之后，取消是一条中性
       * 提示，真失败才标红。
       */
      if (message.includes('取消')) {
        setNotice(t('model.cancelled'));
      } else {
        setError(message);
      }
      await refresh();
    } finally {
      setBusy(false);
      setProgress(null);
    }
  };

  const stop = async () => {
    try {
      await cancelModelDownload();
    } catch (cause) {
      setError(errorText(cause, '无法取消下载。'));
    }
  };

  const discard = async () => {
    setBusy(true);
    setError('');
    setNotice('');
    try {
      setStatus(await removeModel());
      setNotice(t('model.removed'));
    } catch (cause) {
      setError(errorText(cause, '无法删除模型文件。'));
    } finally {
      setBusy(false);
    }
  };

  /**
   * 「引擎」那一行的文字。
   *
   * 三种情况必须分开说，因为用户的下一步动作完全不同：
   *   - 引擎可用 → 说明会自动启动，不需要他做任何事；
   *   - 引擎缺失 → 告诉他要指定路径（或装一份）；
   *   - 硬件跑不动 → 说明本机就不适合，不必折腾引擎。
   *
   * 之前这里是一句写死的话（「启动器不会替你启动它」）。在实现了按需启动之后，
   * 那句话就从「诚实的免责声明」变成了「错误的说明」。
   */
  const runtimeLine = (() => {
    if (engine?.feasibility.disableModel) return t('model.runtimeNotViable');
    if (!engine) return t('model.runtimeUnknown');
    if (!engine.modelInstalled) return t('model.runtimeNeedModel');
    if (engine.runtime.available) {
      return engine.runtime.managedRunning
        ? t('model.runtimeRunning', { values: { path: engine.runtime.path ?? '' } })
        : t('model.runtimeAuto', { values: { path: engine.runtime.path ?? '' } });
    }
    return t('model.runtimeMissing', { values: { reason: engine.runtime.problem ?? '' } });
  })();

  /** 让用户指定 llama-server 的位置。 */
  const chooseEngine = async () => {
    if (!onChangeServerPath) return;
    try {
      const picked = await pickFile(t('model.runtimePick'));
      if (picked) {
        onChangeServerPath(picked);
        // 路径写进设置后重新读一次状态，让下面那行文字立刻变成新的判断。
        setEngine(await getCrashAiStatus());
      }
    } catch (cause) {
      setError(errorText(cause, t('model.runtimePickFailed')));
    }
  };

  /** 立即停止启动器拉起的引擎（归还内存）。 */
  const stopEngine = async () => {
    try {
      await stopCrashAiEngine();
      setEngine(await getCrashAiStatus());
    } catch (cause) {
      setError(errorText(cause, '无法停止推理引擎。'));
    }
  };

  const installed = status?.installed ?? false;
  /** 文件在但不是完整大小：多半是上次中断留下的。 */
  const partial = !installed && (status?.bytesOnDisk ?? 0) > 0;
  const downloading = busy || status?.downloading === true;
  const percent = progress && progress.total > 0
    ? Math.min(100, Math.round((progress.received / progress.total) * 100))
    : 0;

  return (
    <div className={styles.card}>
      <div className={styles.head}>
        <span className={styles.icon}><HardDriveDownload size={16} strokeWidth={1.8} /></span>
        <div className={styles.headCopy}>
          <strong>{t('model.sectionTitle')}</strong>
          <p>{t('model.sectionHint')}</p>
        </div>
        <span className={`${styles.badge} ${installed ? styles.badgeOk : styles.badgeWaiting}`}>
          {installed ? <CheckCircle2 size={12} /> : <Download size={12} />}
          {installed ? t('model.installed') : partial ? t('model.partial') : t('model.missing')}
        </span>
      </div>

      <dl className={styles.facts}>
        <div>
          <dt>{t('model.name')}</dt>
          <dd>{status?.name ?? 'LFM3-1.7B'}</dd>
        </div>
        <div>
          <dt>{t('model.size')}</dt>
          <dd>{t('model.sizeValue', { values: { size: formatBytes(status?.expectedBytes ?? 0) } })}</dd>
        </div>
        {installed && (
          <div className={styles.factWide}>
            <dt>{t('model.location')}</dt>
            <dd className={styles.mono} title={status?.path}>{status?.path}</dd>
          </div>
        )}
      </dl>

      {/*
        未下载时的说明。写在这里而不是放在按钮的 title 里：用户要判断的是「这个
        东西值不值得下 1.1 GB」，而那段判断需要完整的句子。
      */}
      {!installed && <p className={styles.hint}>{t('model.missingHint')}</p>}

      {/*
        进度区。三个阶段的措辞不同：测速、下载、校验。
        把三个阶段混成一句「下载中」会让最后那 1.1 GB 的哈希计算看起来像卡住了。
      */}
      {downloading && (
        <div className={styles.progressBlock}>
          <div className={styles.progressHead}>
            <span>
              {progress?.stage === 'probing'
                ? t('model.probing')
                : progress?.stage === 'verifying'
                  ? t('model.verifying')
                  : t('model.downloading', { values: { source: progress?.source ?? '…' } })}
            </span>
            {progress?.stage === 'downloading' && (
              <span className={styles.progressNumbers}>
                {formatBytes(progress.received)} / {formatBytes(progress.total)} · {percent}%
              </span>
            )}
          </div>
          <div
            className={styles.progressTrack}
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent}
          >
            <div
              className={`${styles.progressFill} ${progress?.stage === 'downloading' ? '' : styles.progressIndeterminate}`}
              style={progress?.stage === 'downloading' ? { width: `${percent}%` } : undefined}
            />
          </div>
          {progress?.bytesPerSecond ? (
            <span className={styles.speed}>
              {t('model.sourceSpeed', { values: { speed: formatBytes(progress.bytesPerSecond) } })}
            </span>
          ) : null}
        </div>
      )}

      {/*
        源列表。
        只在「还没下载」且已经测过速时显示。每项都标出实测速度；全部不可用时
        也给出一句总结，而不是留一串灰掉的条目让用户自己拼。
      */}
      {sources.length > 0 && !installed && (
        <div className={styles.sources}>
          <span className={styles.sourcesTitle}>{t('model.sources')}</span>
          <ul className={styles.sourceList}>
            {sources.map((source, index) => (
              <li key={source.url} className={styles.sourceItem}>
                <span className={styles.sourceName}>
                  {source.label}
                  {index === 0 && source.bytesPerSecond !== null && (
                    <span className={styles.fastest}>{t('model.fastest')}</span>
                  )}
                </span>
                {source.bytesPerSecond !== null ? (
                  <span className={styles.sourceSpeed}>
                    {t('model.sourceSpeed', { values: { speed: formatBytes(source.bytesPerSecond) } })}
                  </span>
                ) : (
                  <span className={styles.sourceProblem}>
                    {t('model.probeFailed', { values: { reason: source.problem ?? t('model.sourceUnavailable') } })}
                  </span>
                )}
              </li>
            ))}
          </ul>
          {sources.every(source => source.bytesPerSecond === null) && (
            <p className={styles.warn}><AlertTriangle size={13} /> {t('model.allSourcesFailed')}</p>
          )}
        </div>
      )}

      {/*
        引擎状态行只在桌面版显示。
        浏览器预览里 `getCrashAiStatus` 返回 null（没有后端可问），一直显示
        「正在检查…」会让人以为它在卡住。预览本来也没有真实引擎可言。
      */}
      {isDesktopShell() && (
        <p className={styles.runtime}>
          <Cpu size={13} /> {runtimeLine}
        </p>
      )}

      {/*
        引擎相关的两个操作。
        只在「模型已下载」时出现：模型都没有的时候，配引擎没有任何意义。
        用 ghost 按钮而不是醒目主色：它们是补救手段，正常路径上不该抢「下载模型」
        或「打开日志目录」的注意力。
      */}
      {installed && isDesktopShell() && (
        <div className={styles.engineActions}>
          {onChangeServerPath && (
            <Button size="sm" variant="ghost" icon={<Cpu size={13} />} onClick={() => void chooseEngine()}>
              {serverPath ? t('model.runtimeChange') : t('model.runtimeChoose')}
            </Button>
          )}
          {engine?.runtime.managedRunning && (
            <Button size="sm" variant="ghost" onClick={() => void stopEngine()}>
              {t('model.runtimeStop')}
            </Button>
          )}
          {engine?.runtime.available && (
            <span className={styles.enginePath} title={engine.runtime.path ?? ''}>
              {t('model.runtimeSource', { values: { source: engine.runtime.source ?? '' } })}
            </span>
          )}
        </div>
      )}

      {!isDesktopShell() && <p className={styles.warn}>{t('model.previewOnly')}</p>}
      {notice && <p className={styles.notice} role="status">{notice}</p>}
      {error && <p className={styles.warn}><AlertTriangle size={13} /> {error}</p>}

      <div className={styles.actions}>
        {downloading ? (
          <Button variant="ghost" onClick={() => void stop()}>{t('model.cancel')}</Button>
        ) : (
          <>
            {/*
              未测速时先给「测速」，测完再给「下载」——两步分开，用户能看到下载
              将从哪个源开始，而不是点一下之后才知道源是慢的那个。
            */}
            {!installed && sources.length === 0 && (
              <Button variant="ghost" icon={<Download size={13} />} onClick={() => void probe()}>
                {t('model.probe')}
              </Button>
            )}
            <Button
              icon={<Download size={13} />}
              loading={busy}
              disabled={!isDesktopShell() || downloading}
              onClick={() => void startDownload()}
            >
              {installed ? t('model.redownload') : t('model.download')}
            </Button>
            {(installed || partial) && (
              <Button variant="ghost" icon={<Trash2 size={13} />} disabled={busy} onClick={() => void discard()}>
                {t('model.remove')}
              </Button>
            )}
          </>
        )}
      </div>
    </div>
  );
}
