import React, { useId } from 'react';
import styles from './Input.module.css';

export interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  label?: string;
  error?: string;
  hint?: string;
  icon?: React.ReactNode;
  iconPosition?: 'left' | 'right';
}

export const Input: React.FC<InputProps> = ({
  label,
  error,
  hint,
  icon,
  iconPosition = 'left',
  className = '',
  id,
  'aria-describedby': ariaDescribedBy,
  'aria-labelledby': ariaLabelledBy,
  'aria-errormessage': ariaErrorMessage,
  'aria-invalid': ariaInvalid,
  ...props
}) => {
  const generatedId = useId();
  const inputId = id ?? generatedId;
  const labelId = `${generatedId}-label`;
  const errorId = `${generatedId}-error`;
  const hintId = `${generatedId}-hint`;
  const describedBy = [ariaDescribedBy, error ? errorId : hint ? hintId : undefined].filter(Boolean).join(' ') || undefined;
  const labelledBy = ariaLabelledBy ?? (label && !props['aria-label'] ? labelId : undefined);
  const errorMessage = [ariaErrorMessage, error ? errorId : undefined].filter(Boolean).join(' ') || undefined;
  const inputClasses = [
    styles.input,
    error && styles.error,
    icon && iconPosition === 'left' && styles.hasLeftIcon,
    icon && iconPosition === 'right' && styles.hasRightIcon,
  ].filter(Boolean).join(' ');

  return (
    <div className={`${styles.wrapper} ${className}`}>
      {label && (
        <label id={labelId} className={styles.label} htmlFor={inputId}>
          {label}
        </label>
      )}
      <div className={styles.inputContainer}>
        {icon && iconPosition === 'left' && (
          <span className={`${styles.icon} ${styles.leftIcon}`}>{icon}</span>
        )}
        <input
          className={inputClasses}
          {...props}
          id={inputId}
          aria-labelledby={labelledBy}
          aria-describedby={describedBy}
          aria-errormessage={errorMessage}
          aria-invalid={error ? true : ariaInvalid}
        />
        {icon && iconPosition === 'right' && (
          <span className={`${styles.icon} ${styles.rightIcon}`}>{icon}</span>
        )}
      </div>
      {error && <div id={errorId} className={styles.errorText}>{error}</div>}
      {hint && !error && <div id={hintId} className={styles.hint}>{hint}</div>}
    </div>
  );
};
