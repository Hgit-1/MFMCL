import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { X } from 'lucide-react'
import { EXIT_DURATION, reducedMotion } from '../motion'
import { ModalCloseContext } from './modal-close'

/**
 * Native `<dialog>` wrapper. Closing is two-phase (`is-leaving` → `close()`), so the
 * exit animation can play before the top-layer element disappears; with reduced
 * motion it closes immediately. The dismissal is published through
 * `ModalCloseContext` so nested forms animate out instead of unmounting the dialog
 * from the parent.
 */
export function Dialog({ title, onClose, children, dismissible = true }: { title: string; onClose: () => void; children: ReactNode; dismissible?: boolean }) {
  const container = useRef<HTMLDialogElement>(null)
  const restore = useRef<Element | null>(null)
  const timer = useRef(0)
  const closing = useRef(false)
  const [leaving, setLeaving] = useState(false)

  const dismiss = useCallback(() => {
    if (!dismissible || closing.current) return
    closing.current = true
    const finish = () => {
      // `close()` is what removes the element from the top layer; the parent then unmounts it.
      if (container.current?.open) container.current.close()
      onClose()
    }
    if (reducedMotion()) { finish(); return }
    setLeaving(true)
    timer.current = window.setTimeout(finish, EXIT_DURATION)
  }, [dismissible, onClose])

  useEffect(() => {
    const dialog = container.current
    restore.current = document.activeElement
    dialog?.showModal()
    return () => {
      window.clearTimeout(timer.current)
      if (dialog?.open) dialog.close()
      if (restore.current instanceof HTMLElement) restore.current.focus()
    }
  }, [])

  return <dialog
    className={`native-dialog ${leaving ? 'is-leaving' : ''}`}
    ref={container}
    onCancel={(e) => { e.preventDefault(); dismiss() }}
    onClick={(e) => { if (e.target === e.currentTarget) dismiss() }}
    aria-label={title}
  >
    <header className="native-dialog-heading"><h2>{title}</h2><button autoFocus disabled={!dismissible} className="icon-button" onClick={dismiss} aria-label="关闭"><X size={20} /></button></header>
    <ModalCloseContext.Provider value={dismiss}>{children}</ModalCloseContext.Provider>
  </dialog>
}
