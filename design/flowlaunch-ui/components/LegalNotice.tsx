// 首次启动的用户协议确认。
//
// ## 展示什么
//
// 三段，按用户实际需要判断的顺序：这是第三方启动器、不是官方产品；它需要正版
// 账号才能下载游戏；它不与 Mojang 或 Microsoft 关联。加上 Minecraft EULA 与
// 使用条款的原文链接。
//
// ## 为什么挡住主界面
//
// PCL2 在未同意时直接退出程序；这里选择挡住界面而不是退出——退出会让「我点错了
// 拒绝」变成一次重启，而挡住之后重开一次即可。两种做法都满足「未同意就不使用」，
// 这个更少惩罚误操作。

import { useState } from 'react';
import { ExternalLink, ScrollText } from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import styles from './LegalNotice.module.css';

interface LegalNoticeProps {
  onAccept: () => void;
}

const LINKS = {
  eula: 'https://www.minecraft.net/eula',
  terms: 'https://www.minecraft.net/usage-guidelines',
  brand: 'https://www.minecraft.net/brand-guidelines',
} as const;

export function LegalNotice({ onAccept }: LegalNoticeProps) {
  const { t } = useI18n();
  const [declined, setDeclined] = useState(false);

  return (
    <div className={styles.overlay} role="dialog" aria-modal="true" aria-labelledby="legal-title">
      <div className={styles.shell}>
        <header className={styles.head}>
          <span className={styles.icon}><ScrollText size={20} strokeWidth={1.8} /></span>
          <div>
            <h1 id="legal-title">{t('legal.title')}</h1>
            <p>{t('legal.subtitle')}</p>
          </div>
        </header>

        <div className={styles.body}>
          {/*
            三条要点各自成段，不合并成一大段免责声明：用户真正会读的是分行短句，
            一整块法律文本只会被跳过。
          */}
          <ul className={styles.points}>
            <li>{t('legal.unofficial')}</li>
            <li>{t('legal.ownership')}</li>
            <li>{t('legal.noAffiliation')}</li>
            <li>{t('legal.offlineScope')}</li>
          </ul>

          <div className={styles.links}>
            <a href={LINKS.eula} target="_blank" rel="noreferrer noopener">
              {t('legal.eulaLink')} <ExternalLink size={12} />
            </a>
            <a href={LINKS.terms} target="_blank" rel="noreferrer noopener">
              {t('legal.termsLink')} <ExternalLink size={12} />
            </a>
            <a href={LINKS.brand} target="_blank" rel="noreferrer noopener">
              {t('legal.brandLink')} <ExternalLink size={12} />
            </a>
          </div>

          {declined && (
            <p className={styles.declined} role="alert">{t('legal.declined')}</p>
          )}
        </div>

        <footer className={styles.actions}>
          <Button variant="ghost" onClick={() => setDeclined(true)}>
            {t('legal.decline')}
          </Button>
          <Button onClick={onAccept}>{t('legal.accept')}</Button>
        </footer>
      </div>
    </div>
  );
}
