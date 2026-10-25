import { createContext, useContext } from 'react'

/**
 * Overlays animate out before they unmount, so the component that closes a dialog
 * must ask the dialog for its dismissal instead of clearing parent state directly.
 * Dialogs opened outside a `Modal` simply fall back to their own `onClose`.
 */
export const ModalCloseContext = createContext<(() => void) | null>(null)

export function useModalClose(fallback: () => void): () => void {
  return useContext(ModalCloseContext) ?? fallback
}
