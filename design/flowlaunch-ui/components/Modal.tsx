import React, { useEffect, useRef } from 'react';
import { useI18n } from '../utils/i18n';
import styles from './Modal.module.css';

export interface ModalProps {
  isOpen: boolean;
  onClose: () => void;
  title?: string;
  size?: 'sm' | 'md' | 'lg' | 'xl';
  children: React.ReactNode;
  footer?: React.ReactNode;
  closeOnOverlay?: boolean;
}

export const Modal: React.FC<ModalProps> = ({
  isOpen,
  onClose,
  title,
  size = 'md',
  children,
  footer,
  closeOnOverlay = true,
}) => {
  const modalRef = useRef<HTMLDivElement>(null);
  const { t } = useI18n();

  /*
   * `onClose` 存进 ref，effect 只依赖 `isOpen`。
   *
   * ## 这一条修的是一个真实的输入故障
   *
   * 之前的依赖是 `[isOpen, onClose]`，而所有调用方都写内联箭头：
   * `<Modal onClose={() => setShowAccountModal(false)}>`。内联箭头每次父组件
   * 渲染都是新函数，于是**父组件一重渲染，这个 effect 就重跑一次**。
   *
   * effect 里有一句 `firstElement?.focus()`，它会把焦点强制挪到对话框里第一个
   * 可聚焦元素——在这个对话框里是右上角的关闭按钮。后果是：在「离线模式」的
   * 用户名输入框里打第一个字，`setOfflineName` 触发父组件重渲染 → effect 重跑
   * → 焦点被抢到关闭按钮 → 后续字符全部丢失。实测输入框里的值永远停在第一个
   * 字符，看起来就是「没办法流畅地输入文本」。
   *
   * 用 ref 之后 effect 只在打开/关闭时跑，输入过程完全不受影响。这也顺带解决了
   * 两个隐患：`document.body.style.overflow` 被反复清空又设回（页面滚动位置会
   * 跳）、`keydown` 监听反复解绑重绑。
   */
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    if (!isOpen) return;
    /*
     * 焦点落点：优先输入框，其次对话框本身。
     *
     * 之前一律聚焦「第一个可聚焦元素」，而它恰恰是关闭按钮——用户打开一个要
     * 填表单的对话框，第一件事却是把光标放在关闭按钮上，按一下回车就把窗口关
     * 掉了。优先给输入框才能「打开即输入」。
     *
     * 没有输入框时聚焦对话框容器（`tabIndex={-1}`）：读屏会读出标题，而不会
     * 像聚焦按钮那样把按钮文字当成对话框的自我介绍。
     */
    const dialog = modalRef.current;
    if (dialog) {
      const input = dialog.querySelector<HTMLElement>(
        'input:not([type="hidden"]):not(:disabled), textarea:not(:disabled), select:not(:disabled)',
      );
      (input ?? dialog).focus();
    }

    const handleEscape = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onCloseRef.current();
    };

    document.addEventListener('keydown', handleEscape);
    document.body.style.overflow = 'hidden';

    return () => {
      document.removeEventListener('keydown', handleEscape);
      document.body.style.overflow = '';
    };
  }, [isOpen]);

  if (!isOpen) return null;

  const handleOverlayClick = (e: React.MouseEvent) => {
    if (closeOnOverlay && e.target === e.currentTarget) {
      onClose();
    }
  };

  return (
    <div className={styles.overlay} onClick={handleOverlayClick}>
      <div
        ref={modalRef}
        className={`${styles.modal} ${styles[size]}`}
        role="dialog"
        aria-modal="true"
        /* 可被脚本聚焦（焦点落点），但不进入 Tab 序列。 */
        tabIndex={-1}
      >
        {title && (
          <div className={styles.header}>
            <h2 className={styles.title}>{title}</h2>
            <button
              className={styles.closeButton}
              onClick={onClose}
              aria-label={t('common.close')}
            >
              <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
                <path d="M15 5L5 15M5 5l10 10" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              </svg>
            </button>
          </div>
        )}
        <div className={styles.content}>
          {children}
        </div>
        {footer && (
          <div className={styles.footer}>
            {footer}
          </div>
        )}
      </div>
    </div>
  );
};
