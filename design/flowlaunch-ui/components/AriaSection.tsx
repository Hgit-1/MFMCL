// 设置页里的「Aria with You」区块。
//
// ## 为什么季节选择是四个按钮而不是下拉
//
// 这个设置的用途是「我想看哪个季节的立绘」。四个选项各自对应一张可以立刻看到的
// 图，所以按钮上带缩略预览、点一下立刻生效——下拉会把「看到」推迟到选中之后，
// 而这个设置的全部意义就是看。
//
// ## 为什么南北半球单独一个开关
//
// 启动器不掌握用户所在地，也只能从系统区域设置里猜。猜错一次的代价是半年里
// 每一天都显示错季节，而用户很难把「立绘不对」归因到这个问题上。与其猜，不如
// 给一个明确的开关，并把「现在是自动识别到哪个季节」写在旁边。

import { useState } from 'react';
import { CalendarDays, Clock, Palette, Sparkles } from 'lucide-react';
import { useI18n } from '../utils/i18n';
import {
  SEASONS,
  seasonArt,
  SEASON_LABEL_KEYS,
  nextSeasonChange,
  seasonAt,
  type AriaArtwork,
  type AriaSetting,
} from '../utils/aria';
import styles from './AriaSection.module.css';

interface AriaSectionProps {
  artwork?: AriaArtwork | null;
  setting: AriaSetting;
  onChange: (next: Partial<AriaSetting>) => void;
  /** 插件总开关。与插件页那个开关是同一个状态，见 App 的说明。 */
  pluginEnabled: boolean;
  /** 当前主题，用于预览框底色。 */
  dark: boolean;
}

function SeasonPreview({ src }: { src: string }) {
  const [failed, setFailed] = useState(false);
  if (failed) return null;
  return <img src={src} alt="" draggable={false} loading="lazy" onError={() => setFailed(true)} />;
}

export function AriaSection({ artwork, setting, onChange, pluginEnabled, dark }: AriaSectionProps) {
  const { t, language } = useI18n();
  const available = Boolean(seasonArt(artwork, setting.fixedSeason));
  const today = new Date();
  const detected = seasonAt(today, setting.southernHemisphere);
  const next = nextSeasonChange(today, setting.southernHemisphere);

  /*
   * 日期的本地化。
   *
   * `Intl.DateTimeFormat` 而不是手拼 `${m}月${d}日`：日语用「3月1日」、英语用
   * 「Mar 1」，三套语言各自有正确写法，而这只是给一句话用的，不值得为它加三个
   * 词典条目。
   */
  const dateText = new Intl.DateTimeFormat(
    language === 'zh-CN' ? 'zh-CN' : language === 'ja' ? 'ja-JP' : 'en-US',
    { month: 'short', day: 'numeric' },
  ).format(next.date);

  return (
    <section className={styles.group}>
      <h3>
        {t('aria.sectionTitle')}
        <span className={styles.badge}>{t('aria.pluginName')}</span>
      </h3>
      <p className={styles.lead}>{t('aria.lead')}</p>

      {/* 插件总开关。它与插件页的开关是同一份状态。 */}
      <div className={styles.masterRow}>
        <label className={styles.switchRow}>
          <input
            type="checkbox"
            checked={available && pluginEnabled}
            disabled={!available}
            onChange={event => onChange({ enabled: event.target.checked })}
          />
          <div>
            <strong>{t('aria.enable')}</strong>
            <span>{t('aria.enableHint')}</span>
          </div>
        </label>
      </div>

      {/*
        以下控件在插件关闭时不可用，但仍然显示。
        把它们藏起来会让用户以为设置丢了；灰掉并显示原因，用户知道「打开开关
        就能用」。
      */}
      <fieldset className={styles.body} disabled={!available || !pluginEnabled}>
        {/* --- 季节来源 ------------------------------------------------- */}
        <div className={styles.row}>
          <div className={styles.rowCopy}>
            <strong><CalendarDays size={12} /> {t('aria.source')}</strong>
            <span>{t('aria.sourceHint')}</span>
          </div>
          <div className={styles.segmented}>
            <button
              type="button"
              className={`${styles.segment} ${setting.mode === 'auto' ? styles.segmentActive : ''}`}
              onClick={() => onChange({ mode: 'auto' })}
            >
              {t('aria.modeAuto')}
            </button>
            <button
              type="button"
              className={`${styles.segment} ${setting.mode === 'fixed' ? styles.segmentActive : ''}`}
              onClick={() => onChange({ mode: 'fixed' })}
            >
              {t('aria.modeFixed')}
            </button>
          </div>
        </div>

        {/*
          自动模式：说明「现在识别到的是哪个季节、下一次换季是什么时候」。
          这行文字是插件诚实的证据——用户不需要相信它，可以对照日历检查。
        */}
        {setting.mode === 'auto' ? (
          <p className={styles.note}>
            <Clock size={11} />
            {t('aria.detected', {
              values: {
                season: t(SEASON_LABEL_KEYS[detected]),
                date: dateText,
                next: t(SEASON_LABEL_KEYS[next.season]),
              },
            })}
          </p>
        ) : (
          /* 固定模式：四个可见按钮，每个带缩略图。 */
          <div className={styles.seasons}>
            {SEASONS.map(season => {
              const art = pluginEnabled ? seasonArt(artwork, season) : null;
              return (
                <button
                  key={season}
                  type="button"
                  className={`${styles.season} ${setting.fixedSeason === season ? styles.seasonActive : ''}`}
                  onClick={() => onChange({ fixedSeason: season })}
                  title={t(SEASON_LABEL_KEYS[season])}
                >
                  <span className={styles.seasonThumb} data-dark={dark ? 'true' : undefined}>
                    {art && (
                      <SeasonPreview key={`${artwork?.assetBase}-${artwork?.version}-${season}`} src={art.thumb} />
                    )}
                  </span>
                  <span>{t(SEASON_LABEL_KEYS[season])}</span>
                </button>
              );
            })}
          </div>
        )}

        {/* --- 半球 ------------------------------------------------------ */}
        <label className={styles.switchRow}>
          <input
            type="checkbox"
            checked={setting.southernHemisphere}
            onChange={event => onChange({ southernHemisphere: event.target.checked })}
          />
          <div>
            <strong>{t('aria.southern')}</strong>
            <span>{t('aria.southernHint')}</span>
          </div>
        </label>

        {/* --- 显示位置 -------------------------------------------------- */}
        <div className={styles.row}>
          <div className={styles.rowCopy}>
            <strong><Sparkles size={12} /> {t('aria.placement')}</strong>
            <span>{t('aria.placementHint')}</span>
          </div>
        </div>
        <label className={styles.switchRow}>
          <input
            type="checkbox"
            checked={setting.showHero}
            onChange={event => onChange({ showHero: event.target.checked })}
          />
          <div>
            <strong>{t('aria.showHero')}</strong>
            <span>{t('aria.showHeroHint')}</span>
          </div>
        </label>
        <label className={styles.switchRow}>
          <input
            type="checkbox"
            checked={setting.showSidebar}
            onChange={event => onChange({ showSidebar: event.target.checked })}
          />
          <div>
            <strong>{t('aria.showSidebar')}</strong>
            <span>{t('aria.showSidebarHint')}</span>
          </div>
        </label>
        <label className={styles.switchRow}>
          <input
            type="checkbox"
            checked={setting.showWatermark}
            onChange={event => onChange({ showWatermark: event.target.checked })}
          />
          <div>
            <strong>{t('aria.showWatermark')}</strong>
            <span>{t('aria.showWatermarkHint')}</span>
          </div>
        </label>

        {/* --- 染色 ------------------------------------------------------ */}
        <label className={styles.switchRow}>
          <input
            type="checkbox"
            checked={setting.tintSurfaces}
            onChange={event => onChange({ tintSurfaces: event.target.checked })}
          />
          <div>
            <strong><Palette size={12} /> {t('aria.tint')}</strong>
            <span>{t('aria.tintHint')}</span>
          </div>
        </label>
      </fieldset>

      {/*
        素材来源说明。
        这几张图是生成出来的，用户有权知道它们不是手工绘制的；同时说明图片会随
        插件一起分发、运行时不会联网——这正是插件清单里刻意不申请 `network`
        权限的原因，写在界面上比藏在清单里更有用。
      */}
      <p className={styles.footnote}>{t('aria.assetNote')}</p>
    </section>
  );
}
