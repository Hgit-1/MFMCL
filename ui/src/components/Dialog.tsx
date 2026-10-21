import { useEffect, useRef, type ReactNode } from 'react'
import { X } from 'lucide-react'

export function Dialog({ title, onClose, children, dismissible = true }: { title: string; onClose: () => void; children: ReactNode; dismissible?: boolean }) {
  const container = useRef<HTMLDialogElement>(null)
  const restore = useRef<Element | null>(null)
  useEffect(() => {
    const dialog = container.current
    restore.current = document.activeElement
    dialog?.showModal()
    return () => {
      dialog?.close()
      if (restore.current instanceof HTMLElement) restore.current.focus()
    }
  }, [])
  return <dialog className="native-dialog" ref={container} onCancel={(e) => { e.preventDefault(); if (dismissible) onClose() }} onClick={(e) => { if (dismissible && e.target === e.currentTarget) onClose() }} aria-label={title}>
    <header className="native-dialog-heading"><h2>{title}</h2><button autoFocus disabled={!dismissible} className="icon-button" onClick={onClose} aria-label="关闭"><X size={20} /></button></header>
    {children}
  </dialog>
}
