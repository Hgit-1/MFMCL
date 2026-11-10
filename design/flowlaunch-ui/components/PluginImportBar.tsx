// 插件页顶部的「导入 / 来源」操作条。
//
// ## 为什么单独一个组件
//
// `PluginWorkspace` 已经有六百多行，而这条操作条自己有一整套状态机：文件选择、
// URL 输入、正在安装、失败原因、安装成功后交回父组件的记录。塞进页面组件会让
// 「列表怎么渲染」和「包装到哪了」互相牵制，而它们没有任何关系。
//
// ## 三条入口，一件共同的事
//
// 本机文件、HTTPS 网址、插件源里的条目，最终都走同一个后端安装函数。区别只在
// 用户手里有什么：一个下载好的包、一个作者给的链接、或者什么都没有只想逛逛。
// 所以三个入口并列，而不是做成「安装」下的一层菜单——多一层点击换来的是用户
// 要猜「我的包算哪种」。

import { useRef, useState } from 'react';
import { Download, FolderOpen, Link2, RefreshCw, Store } from 'lucide-react';
import { useI18n } from '../utils/i18n';
import { errorText, isDesktopShell } from '../utils/native';
import {
  installPluginFromFile,
  installPluginFromUrl,
  validateSourceUrl,
  normalizeSourceUrl,
  type InstalledPluginRecord,
} from '../utils/pluginInstall';
import styles from './PluginWorkspace.module.css';

interface PluginImportBarProps {
  onNotice: (message: string) => void;
  /** 安装成功后交回记录，由页面负责弹确认框。 */
  onInstalled: (record: InstalledPluginRecord) => void;
  onOpenSourcePanel: () => void;
  onReload: () => void;
}

export function PluginImportBar({
  onNotice,
  onInstalled,
  onOpenSourcePanel,
  onReload,
}: PluginImportBarProps) {
  const { t } = useI18n();
  const [url, setUrl] = useState('');
  const [urlOpen, setUrlOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const desktop = isDesktopShell();

  /**
   * 打开本机文件选择。
   *
   * 用隐藏的 `<input type="file">` 而不是 Tauri 的对话框插件：文件内容要交给
   * 后端按字节读，而 `<input>` 给出的路径在桌面外壳里是真实路径。走对话框插件
   * 要多一条 capability 授权，换来的只是「看起来更像原生」——不值。
   */
  const pickFile = () => {
    if (!desktop) {
      onNotice(t('plugin.installInvalid'));
      return;
    }
    fileRef.current?.click();
  };

  const installFile = async (file: File) => {
    setBusy(true);
    try {
      /*
       * 桌面外壳里 `<input type="file">` 的 `File` 带 `path`，这是 Tauri 注入的。
       * 浏览器里没有，所以这条分支只在桌面走得到。
       */
      const path = (file as File & { path?: string }).path;
      if (!path) throw new Error(t('plugin.importInvalidUrl'));
      onInstalled(await installPluginFromFile(path));
    } catch (error) {
      onNotice(errorText(error, t('plugin.installInvalid')));
    } finally {
      setBusy(false);
      if (fileRef.current) fileRef.current.value = '';
    }
  };

  const installUrl = async () => {
    const candidate = normalizeSourceUrl(url);
    const problem = validateSourceUrl(candidate);
    if (problem) {
      onNotice(problem);
      return;
    }
    setBusy(true);
    try {
      onInstalled(await installPluginFromUrl(candidate));
      setUrl('');
      setUrlOpen(false);
    } catch (error) {
      onNotice(errorText(error, t('plugin.installInvalid')));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.importBar}>
      <input
        ref={fileRef}
        type="file"
        accept=".zip,.mflp,.mfplugin"
        className={styles.hiddenInput}
        onChange={event => {
          const file = event.target.files?.[0];
          if (file) void installFile(file);
        }}
      />
      <button type="button" className={styles.importAction} onClick={pickFile} disabled={busy}>
        <FolderOpen size={13} /> {t('plugin.importPackage')}
      </button>
      <button
        type="button"
        className={styles.importAction}
        onClick={() => setUrlOpen(open => !open)}
        disabled={busy}
        aria-expanded={urlOpen}
      >
        <Link2 size={13} /> {t('plugin.importFromUrl')}
      </button>
      <span className={styles.importDivider} aria-hidden="true" />
      <button type="button" className={styles.importAction} onClick={onOpenSourcePanel} disabled={busy}>
        <Store size={13} /> {t('plugin.sourceBrowse')}
      </button>
      <button type="button" className={styles.importAction} onClick={onReload} disabled={busy}>
        <RefreshCw size={13} /> {t('plugin.reload')}
      </button>
      {busy && <span className={styles.importBusy} role="status"><Download size={12} /> {t('plugin.importing')}</span>}

      {/*
        URL 输入折在按钮下面而不是另开弹窗：装一个包是一个短动作，弹窗要额外一次
        关闭操作，而这里的输入框只有一行。展开后按回车即安装，失败提示走页面顶部
        的 toast（与安装清单失败同一处），不需要在这里再叠一层错误排版。
      */}
      {urlOpen && (
        <div className={styles.importUrlRow}>
          <input
            value={url}
            onChange={event => setUrl(event.target.value)}
            onKeyDown={event => { if (event.key === 'Enter') void installUrl(); }}
            placeholder={t('plugin.importUrlPlaceholder')}
            aria-label={t('plugin.importFromUrl')}
            spellCheck={false}
            autoFocus
          />
          <button type="button" onClick={() => void installUrl()} disabled={busy || !url.trim()}>
            {t('plugin.importConfirm')}
          </button>
          <p>{t('plugin.importFromUrlHint')}</p>
        </div>
      )}
    </div>
  );
}
