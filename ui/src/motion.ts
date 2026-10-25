/**
 * Motion helpers shared by the shell and its pages.
 *
 * Everything here is best-effort decoration: when the OS asks for reduced motion,
 * or the pointer is not a fine pointer (touch), the helpers do nothing and the
 * CSS falls back to a static presentation.
 */
import { useCallback, useEffect, useRef, useState } from 'react'

export const reducedMotion = () =>
  typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches

/**
 * Pointer parallax for a container: writes `--tilt-x` / `--tilt-y` custom
 * properties that CSS consumes with different multipliers per layer, so one
 * listener drives the whole scene. Values are intentionally small (≤ 8px).
 */
export function attachPointerParallax(element: HTMLElement, strength = 8) {
  if (reducedMotion() || !window.matchMedia('(pointer: fine)').matches) return () => undefined
  let frame = 0
  const reset = () => {
    cancelAnimationFrame(frame)
    element.style.setProperty('--tilt-x', '0px')
    element.style.setProperty('--tilt-y', '0px')
  }
  const move = (event: PointerEvent) => {
    cancelAnimationFrame(frame)
    frame = requestAnimationFrame(() => {
      const rect = element.getBoundingClientRect()
      if (!rect.width || !rect.height) return
      const dx = (event.clientX - rect.left) / rect.width - 0.5
      const dy = (event.clientY - rect.top) / rect.height - 0.5
      element.style.setProperty('--tilt-x', `${(dx * strength).toFixed(2)}px`)
      element.style.setProperty('--tilt-y', `${(dy * strength).toFixed(2)}px`)
    })
  }
  element.addEventListener('pointermove', move)
  element.addEventListener('pointerleave', reset)
  element.addEventListener('blur', reset)
  return () => {
    element.removeEventListener('pointermove', move)
    element.removeEventListener('pointerleave', reset)
    element.removeEventListener('blur', reset)
    reset()
  }
}

/** Time an exit animation may run before the element is removed from the tree. */
export const EXIT_DURATION = 180

/**
 * Two-phase dismissal for overlays: mark the element as leaving, let CSS play the
 * exit animation, then run the real close. Without animation support (or with
 * reduced motion) the close happens immediately so behaviour never changes.
 */
export function useExitAnimation() {
  const [leaving, setLeaving] = useState(false)
  const timer = useRef(0)
  const done = useRef(false)
  useEffect(() => () => window.clearTimeout(timer.current), [])
  // Stable identity: callers pass the real close handler when they dismiss, so the
  // hook itself never has to depend on a parent callback that changes each render.
  const dismiss = useCallback((close: () => void) => {
    if (done.current) return
    done.current = true
    if (reducedMotion()) { close(); return }
    setLeaving(true)
    timer.current = window.setTimeout(close, EXIT_DURATION)
  }, [])
  return { leaving, dismiss }
}
