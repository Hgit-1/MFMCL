// 实例文件面板：打开各个内容目录。
//
// ## 为什么做成「打开文件夹」而不是内置文件管理器
//
// 玩家管理模组、存档、资源包时，最自然的工具就是他自己的文件管理器——能拖拽、
// 能多选、能用系统自带的预览和压缩。启动器里再做一个文件管理器只会更差：
// 没有预览、没有拖放、还要为每种文件写一套 UI。
//
// PCL2 与 HMCL 的做法也是同一个思路：给出「打开 XX 文件夹」的入口，把文件操作
// 交给系统。所以这里只做定位，不做编辑。
//
// ## 为什么集中在一处
//
// 这些目录之前散落在页面各处（模组在模组卡片、备份在备份卡片），用户想找
// 「存档在哪」时不知道该看哪一块。集中成一个「实例文件」区，按「会经常动的」
// 到「只在出问题时才看的」排列，找东西时只需看一列。
//
// ## 目录不存在时
//
// 首次安装、从未启动过的实例，`mods` / `saves` / `logs` 这些目录还不存在。
// 后端会在打开前自动创建（根目录与游戏目录除外——那两个不存在说明实例本身有问题，
// 创建空目录只会掩盖错误）。所以界面上不需要区分「存在与否」。

import { useState } from 'react';
import {
  Archive,
  Bug,
  FileText,
  FolderOpen,
  Image,
  Package,
  ScrollText,
  Settings2,
  Sun,
  type LucideIcon,
} from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import {
  errorText,
  exportModpack,
  openInstanceFolder,
  pickDirectory,
  type InstanceFolder,
} from '../utils/native';
import type { GameInstance } from '../types';
import styles from './InstanceFiles.module.css';

interface InstanceFilesProps {
  instance: GameInstance;
  onNotice: (message: string) => void;
}

interface FolderRow {
  folder: InstanceFolder;
  icon: LucideIcon;
  labelKey: 'files.mods' | 'files.saves' | 'files.resourcePacks' | 'files.shaderPacks'
    | 'files.config' | 'files.logs' | 'files.crashReports' | 'files.gameRoot';
  hintKey: 'files.modsHint' | 'files.savesHint' | 'files.resourcePacksHint' | 'files.shaderPacksHint'
    | 'files.configHint' | 'files.logsHint' | 'files.crashReportsHint' | 'files.gameRootHint';
  /** 玩家会频繁打开的目录排在前面。 */
  frequent?: boolean;
}

/**
 * 目录清单。
 *
 * 顺序按「多久会打开一次」排：模组、存档、资源包是日常；配置、光影是偶尔；
 * 日志与崩溃报告只在出问题时看；游戏根目录是兜底入口，放最后。
 */
const FOLDERS: FolderRow[] = [
  { folder: 'mods', icon: Package, labelKey: 'files.mods', hintKey: 'files.modsHint', frequent: true },
  { folder: 'saves', icon: Archive, labelKey: 'files.saves', hintKey: 'files.savesHint', frequent: true },
  { folder: 'resource_packs', icon: Image, labelKey: 'files.resourcePacks', hintKey: 'files.resourcePacksHint', frequent: true },
  { folder: 'shader_packs', icon: Sun, labelKey: 'files.shaderPacks', hintKey: 'files.shaderPacksHint' },
  { folder: 'config', icon: Settings2, labelKey: 'files.config', hintKey: 'files.configHint' },
  { folder: 'logs', icon: ScrollText, labelKey: 'files.logs', hintKey: 'files.logsHint' },
  { folder: 'crash_reports', icon: Bug, labelKey: 'files.crashReports', hintKey: 'files.crashReportsHint' },
  { folder: 'game', icon: FolderOpen, labelKey: 'files.gameRoot', hintKey: 'files.gameRootHint' },
];

export function InstanceFiles({ instance, onNotice }: InstanceFilesProps) {
  const { t } = useI18n();
  const [opening, setOpening] = useState<InstanceFolder | ''>('');

  const open = async (folder: InstanceFolder) => {
    setOpening(folder);
    try {
      const path = await openInstanceFolder(instance.id, folder);
      // 成功时也给出反馈：在某些 Linux 桌面上 xdg-open 会静默失败，
      // 没有提示的话用户无法区分「已打开」和「什么都没发生」。
      onNotice(t('files.opened', { values: { path } }));
    } catch (error) {
      onNotice(errorText(error, t('files.openFailed')));
    } finally {
      setOpening('');
    }
  };

  return (
    <div className={styles.grid}>
      {FOLDERS.map(row => {
        const Icon = row.icon;
        return (
          <button
            key={row.folder}
            type="button"
            className={`${styles.tile} ${row.frequent ? styles.tileFrequent : ''}`}
            onClick={() => void open(row.folder)}
            disabled={opening !== ''}
            title={t(row.hintKey)}
          >
            <span className={styles.tileIcon}><Icon size={16} strokeWidth={1.8} /></span>
            <span className={styles.tileCopy}>
              <strong>{t(row.labelKey)}</strong>
              <small>{t(row.hintKey)}</small>
            </span>
            {opening === row.folder && <span className={styles.tileSpinner} aria-hidden="true" />}
          </button>
        );
      })}
    </div>
  );
}

/**
 * 导出整合包。
 *
 * 单独成一个组件而不是并进上面的目录列表：它的动作性质不同——上面都是「去看」，
 * 这个会**写出一个文件**，而且涉及再分发权利，需要显式确认。
 *
 * 不开放进 `InstanceMaintenance`：那里的两个动作是「检查更新」与「导出」，检查更新
 * 是只读的；把「写入文件」和「读取状态」放在一起，用户容易在没看清的情况下点错。
 */
export function ExportPackCard({ instance, onNotice }: InstanceFilesProps) {
  const { t } = useI18n();
  const [confirmed, setConfirmed] = useState(false);
  const [exporting, setExporting] = useState(false);

  const runExport = async () => {
    if (!confirmed) return;
    const directory = await pickDirectory(t('files.exportPickDir'));
    // 用户取消了目录选择：不算失败，不需要提示。
    if (!directory) return;
    setExporting(true);
    try {
      const path = await exportModpack(instance.id, directory, true);
      onNotice(t('files.exported', { values: { path } }));
    } catch (error) {
      onNotice(errorText(error, t('files.exportFailed')));
    } finally {
      setExporting(false);
    }
  };

  return (
    <div className={styles.exportCard}>
      <div className={styles.exportHead}>
        <span className={styles.tileIcon}><FileText size={16} strokeWidth={1.8} /></span>
        <div>
          <strong>{t('files.exportTitle')}</strong>
          <small>{t('files.exportHint')}</small>
        </div>
      </div>

      {/*
        再分发确认做成常驻复选框：导出按钮在勾选前保持禁用，用户一眼能看出
        必须先确认。用弹窗的话按钮看起来可点，点下去才被告知不可用。
      */}
      <label className={styles.confirm}>
        <input
          type="checkbox"
          checked={confirmed}
          onChange={event => setConfirmed(event.target.checked)}
          disabled={exporting}
        />
        <span>{t('files.exportRights')}</span>
      </label>

      <div className={styles.exportActions}>
        <Button
          size="sm"
          icon={<Archive size={14} />}
          loading={exporting}
          disabled={!confirmed}
          onClick={() => void runExport()}
        >
          {t('files.exportButton')}
        </Button>
      </div>
    </div>
  );
}
