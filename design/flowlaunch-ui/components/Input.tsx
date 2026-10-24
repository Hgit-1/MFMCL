import React from 'react';
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
  ...props
}) => {
  const inputClasses = [
    styles.input,
    error && styles.error,
    icon && iconPosition === 'left' && styles.hasLeftIcon,
    icon && iconPosition === 'right' && styles.hasRightIcon,
  ].filter(Boolean).join(' ');

  return (
    <div className={`${styles.wrapper} ${className}`}>
      {label && (
        <label className={styles.label} htmlFor={props.id}>
          {label}
        </label>
      )}
      <div className={styles.inputContainer}>
        {icon && iconPosition === 'left' && (
          <span className={`${styles.icon} ${styles.leftIcon}`}>{icon}</span>
        )}
        <input className={inputClasses} {...props} />
        {icon && iconPosition === 'right' && (
          <span className={`${styles.icon} ${styles.rightIcon}`}>{icon}</span>
        )}
      </div>
      {error && <div className={styles.errorText}>{error}</div>}
      {hint && !error && <div className={styles.hint}>{hint}</div>}
    </div>
  );
};
