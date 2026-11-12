import React, { useId, useRef } from 'react';
import { useDialogFocus } from '../hooks/useDialogFocus';
import { useI18n } from '../utils/i18n';
import styles from './Modal.module.css';

export interface ModalProps extends Pick<React.AriaAttributes, 'aria-label' | 'aria-labelledby' | 'aria-describedby'> {
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
  'aria-label': ariaLabel,
  'aria-labelledby': ariaLabelledBy,
  'aria-describedby': ariaDescribedBy,
}) => {
  const modalRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const { t } = useI18n();
  const { isTopDialog } = useDialogFocus(modalRef, isOpen, onClose);

  if (!isOpen) return null;

  const handleOverlayClick = (event: React.MouseEvent) => {
    if (closeOnOverlay && event.target === event.currentTarget && isTopDialog()) onClose();
  };

  return (
    <div className={styles.overlay} onClick={handleOverlayClick}>
      <div
        ref={modalRef}
        className={`${styles.modal} ${styles[size]}`}
        role="dialog"
        aria-modal="true"
        aria-label={ariaLabel}
        aria-labelledby={ariaLabelledBy ?? (title ? titleId : undefined)}
        aria-describedby={ariaDescribedBy}
        tabIndex={-1}
      >
        {title && (
          <div className={styles.header}>
            <h2 id={titleId} className={styles.title}>{title}</h2>
            <button
              className={styles.closeButton}
              onClick={() => { if (isTopDialog()) onClose(); }}
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
