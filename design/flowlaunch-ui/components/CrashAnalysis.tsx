// 崩溃日志分析：规则的结论先到，模型的解释随后逐字出现。
//
// ## 这个界面要回答的三个问题
//
//   1. **是什么问题**——规则与堆栈因果分析给结论，4–14 毫秒就有。
//   2. **为什么**——模型解释这段日志里的细节。慢（CPU 上 6–13 秒）。
//   3. **现在怎么办**——只由规则提供。模型给的建议一律不使用（见下）。
//
// ## 为什么必须先显示规则结果
//
// 模型在 CPU 上要跑十几秒。如果等它一起出结果，用户在整段时间里看不到任何东西，
// 分不清「在思考」和「卡死了」。所以规则结论一到就渲染，模型区域单独显示进度，
// 并在生成过程中把文字逐段追加进来。
//
// ## 为什么模型给的动作永远不采用
//
// 实测：给一个 `OutOfMemoryError` 的日志，1.7B 模型建议「把堆内存降到 512 MB」——
// 那会让崩溃立刻且必然地发生。这不是个别失手，小模型对「方向」不可靠。所以步骤
// 永远来自规则，没有规则命中时就不给步骤，只给一句「没有把握」。这条约束写在
// 后端（`decide_steps`），界面只是如实呈现来源。

import { useCallback, useEffect, useRef, useState } from 'react';
import { AlertTriangle, CheckCircle2, Loader2, Sparkles, Square } from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import {
  analyseCrashLogStreaming,
  errorText,
  isDesktopShell,
  onCrashProgress,
  type CrashAnalysis,
  type CrashEnvironment,
  type CrashProgress,
} from '../utils/native';
import styles from './CrashAnalysis.module.css';

interface CrashAnalysisPanelProps {
  /** 要分析的日志正文。为空时禁用按钮。 */
  log: string;
  /** 能提供的上下文（实例的 Java、版本、模组数）。 */
  environment?: CrashEnvironment;
  /** 提示条：分析失败等。 */
  onNotice?: (message: string) => void;
}

/** 阶段。用状态机而不是几个布尔值：它们互斥，组合成一堆 bool 迟早出现矛盾组合。 */
type Stage = 'idle' | 'rules' | 'model' | 'done';

export function CrashAnalysisPanel({ log, environment, onNotice }: CrashAnalysisPanelProps) {
  const { t } = useI18n();
  const [stage, setStage] = useState<Stage>('idle');
  const [analysis, setAnalysis] = useState<CrashAnalysis | null>(null);
  /**
   * 模型已经生成的文字。
   *
   * 与 `analysis.narrative` 分开：后端会在结束时把完整文本放进返回值，而中途
   * 只有增量。两者合并写会让「正在生成」和「已完成」共用同一个字段，重渲染时
   * 容易把已显示的文本清掉又填回来（看着像闪一下）。
   */
  const [streamed, setStreamed] = useState('');
  /** 模型阶段失败的原因。与规则结论无关，规则部分仍然有效。 */
  const [modelError, setModelError] = useState('');

  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);

  // 订阅进度。规则结论先到，模型文字随后追加。
  useEffect(() => {
    let disposed = false;
    const cleanups: Array<() => void> = [];
    void onCrashProgress((payload: CrashProgress) => {
      if (disposed || !alive.current) return;
      if (payload.kind === 'rules') {
        setAnalysis(payload.analysis);
        setStage('model');
        return;
      }
      if (payload.kind === 'chunk') {
        setStreamed(current => current + payload.text);
        return;
      }
      // modelDone
      if (payload.reason) setModelError(payload.reason);
      setStage('done');
    }).then(unlisten => {
      if (unlisten) cleanups.push(unlisten);
    });
    return () => {
      disposed = true;
      cleanups.forEach(off => off());
    };
  }, []);

  const run = useCallback(async () => {
    if (!log.trim()) {
      onNotice?.(t('crash.needLog'));
      return;
    }
    setStage('rules');
    setAnalysis(null);
    setStreamed('');
    setModelError('');
    try {
      const result = await analyseCrashLogStreaming(log, environment, true);
      if (!alive.current) return;
      setAnalysis(result);
      // 结束时以返回值为准：增量可能因为事件丢失而不完整。
      if (result.narrative) setStreamed(result.narrative.text);
      setStage('done');
    } catch (error) {
      if (!alive.current) return;
      setStage('idle');
      onNotice?.(errorText(error, t('crash.failed')));
    }
  }, [log, environment, onNotice, t]);

  const busy = stage === 'rules' || stage === 'model';
  /** 规则结果到手后就能显示原因；模型文字还在长。 */
  const showResult = analysis !== null;
  const modelThinking = stage === 'model' && !modelError;

  return (
    <div className={styles.panel}>
      <div className={styles.head}>
        <div className={styles.headCopy}>
          <strong>{t('crash.title')}</strong>
          <span>{t('crash.hint')}</span>
        </div>
        <Button
          icon={busy ? <Loader2 size={14} className={styles.spin} /> : <Sparkles size={14} />}
          // 分析期间不允许重入：再点一次会与正在跑的模型抢同一个服务。
          disabled={busy || !log.trim()}
          onClick={() => void run()}
        >
          {busy ? t('crash.analysing') : t('crash.analyse')}
        </Button>
      </div>

      {!isDesktopShell() && <p className={styles.note}>{t('crash.previewOnly')}</p>}

      {/*
        进度条只在「规则还没到」时显示不确定动画；规则一到就换成内容。
        这是刻意的：规则的等待是毫秒级，给它设计一个进度条没有意义，但完全没有
        反馈又会让点击看起来没生效。
      */}
      {stage === 'rules' && (
        <div className={styles.progressRow} role="status" aria-live="polite">
          <span className={styles.progressTrack}><i /></span>
          <span className={styles.progressText}>{t('crash.readingLog')}</span>
        </div>
      )}

      {showResult && (
        <div className={styles.result}>
          {/* --- 结论 --- */}
          {analysis!.cause && (
            <div className={styles.cause}>
              <span className={styles.causeTag}>{t('crash.causeTag')}</span>
              <strong>{analysis!.cause.summary}</strong>
              {analysis!.cause.detail && <p>{analysis!.cause.detail}</p>}
            </div>
          )}

          {analysis!.findings.length > 0 ? (
            <ol className={styles.findings}>
              {analysis!.findings.map((finding, index) => (
                <li key={`${finding.id}-${index}`} className={styles.finding}>
                  <div className={styles.findingHead}>
                    <strong>{finding.title}</strong>
                    <span className={`${styles.confidence} ${finding.confidence === 'high' ? styles.confHigh : styles.confMedium}`}>
                      {t(finding.confidence === 'high' ? 'crash.confidenceHigh' : 'crash.confidenceMedium')}
                    </span>
                  </div>
                  <p className={styles.explanation}>{finding.explanation}</p>
                  {finding.suggestions.length > 0 && (
                    <ol className={styles.suggestions}>
                      {finding.suggestions.map((suggestion, i) => <li key={i}>{suggestion}</li>)}
                    </ol>
                  )}
                  {finding.evidence.length > 0 && (
                    <ul className={styles.evidence}>
                      {finding.evidence.slice(0, 4).map((line, i) => <li key={i}>{line}</li>)}
                    </ul>
                  )}
                </li>
              ))}
            </ol>
          ) : (
            <p className={styles.noMatch}>{t('crash.noMatch')}</p>
          )}

          {/*
            模型区域。
            生成过程中显示的文本是逐段追加的，末尾有一个闪烁的光标——它同时回答
            了「还在动吗」和「动到哪了」。用光标而不是进度条：模型输出的长度事先
            未知，一条假装知道百分比的进度条只会说谎。
          */}
          <div className={styles.modelBlock}>
            <div className={styles.modelHead}>
              <span className={styles.modelTag}><Sparkles size={12} />{t('crash.modelTag')}</span>
              {modelThinking && (
                <span className={styles.modelStatus}>
                  <Loader2 size={12} className={styles.spin} />
                  {t('crash.modelThinking')}
                </span>
              )}
              {!modelThinking && modelError && (
                <span className={styles.modelFailed}><AlertTriangle size={12} />{t('crash.modelUnavailable')}</span>
              )}
              {!modelThinking && !modelError && streamed && (
                <span className={styles.modelDone}><CheckCircle2 size={12} />{t('crash.modelDone')}</span>
              )}
            </div>

            {streamed ? (
              <p className={styles.modelText}>
                {streamed}
                {modelThinking && <span className={styles.cursor} aria-hidden="true" />}
              </p>
            ) : modelThinking ? (
              /* 还没有任何 token：用三点跳动表示「正在读日志」，比静止的空白更明确。 */
              <p className={styles.pending}>
                {t('crash.modelPending')}
                <span className={styles.dots} aria-hidden="true"><i /><i /><i /></span>
              </p>
            ) : (
              <p className={styles.pending}>{t('crash.modelSkipped')}</p>
            )}

            {/* 模型失败不影响规则结论；这里必须说清楚，否则用户会以为整次分析失败。 */}
            {modelError && <p className={styles.modelErrorNote}>{modelError}</p>}
            <p className={styles.sourceNote}>{t('crash.stepsFromRules')}</p>
          </div>

          {analysis!.warnings.length > 0 && (
            <ul className={styles.warnings}>
              {analysis!.warnings.map((warning, index) => (
                <li key={index}><AlertTriangle size={12} />{warning}</li>
              ))}
            </ul>
          )}
        </div>
      )}

      {busy && showResult && (
        <div className={styles.footNote}>
          <Square size={11} /> {t('crash.canLeave')}
        </div>
      )}
    </div>
  );
}
