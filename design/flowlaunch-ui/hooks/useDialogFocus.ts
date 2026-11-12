import { useLayoutEffect, useRef, type RefObject } from 'react';

interface DialogEntry {
  element: HTMLElement;
  onClose: () => void;
  restoreFocus: HTMLElement | null;
  lastFocused: HTMLElement | null;
}

const dialogs: DialogEntry[] = [];
const inertBefore = new Map<HTMLElement, string | null>();
let bodyOverflow = '';
let observer: MutationObserver | null = null;
let revision = 0;

const focusableSelector = 'a[href], area[href], button, input:not([type="hidden"]), select, textarea, [tabindex], [contenteditable="true"], audio[controls], video[controls], summary';

function topDialog(): DialogEntry | undefined {
  return dialogs[dialogs.length - 1];
}

function isAvailable(element: HTMLElement): boolean {
  if (!element.isConnected || element.matches(':disabled') || element.closest('[inert], [hidden], [aria-hidden="true"]')) return false;
  const style = getComputedStyle(element);
  return style.display !== 'none' && style.visibility !== 'hidden' && element.getClientRects().length > 0;
}

function focusableElements(dialog: HTMLElement): HTMLElement[] {
  return Array.from(dialog.querySelectorAll<HTMLElement>(focusableSelector))
    .filter(element => element.tabIndex >= 0 && isAvailable(element))
    .sort((left, right) => {
      const leftOrder = left.tabIndex > 0 ? left.tabIndex : Infinity;
      const rightOrder = right.tabIndex > 0 ? right.tabIndex : Infinity;
      return leftOrder === rightOrder ? 0 : leftOrder - rightOrder;
    });
}

function focusDialog(entry: DialogEntry): void {
  const explicit = Array.from(entry.element.querySelectorAll<HTMLElement>('[data-dialog-initial-focus], [autofocus]')).find(isAvailable);
  const preferred = explicit ?? Array.from(entry.element.querySelectorAll<HTMLElement>(
    'input:not([type="hidden"]):not([type="checkbox"]):not([type="radio"]):not([type="button"]):not([type="submit"]), textarea, select',
  )).find(isAvailable);
  const target = entry.lastFocused && isAvailable(entry.lastFocused)
    ? entry.lastFocused
    : preferred ?? entry.element;
  target.focus({ preventScroll: true });
}

function restoreInert(): void {
  for (const [element, value] of inertBefore) {
    if (value === null) element.removeAttribute('inert');
    else element.setAttribute('inert', value);
  }
  inertBefore.clear();
}

function syncInert(): void {
  restoreInert();
  const top = topDialog();
  if (!top?.element.isConnected) return;
  // Inert sibling branches rather than a root: dialogs can be inline or portalled,
  // and a nested dialog must never inherit inert from its own ancestor.
  let branch: HTMLElement = top.element;
  while (branch.parentElement) {
    const parent = branch.parentElement;
    for (const sibling of Array.from(parent.children)) {
      if (sibling === branch || !(sibling instanceof HTMLElement)) continue;
      inertBefore.set(sibling, sibling.getAttribute('inert'));
      sibling.setAttribute('inert', '');
    }
    if (parent === document.body) break;
    branch = parent;
  }
}

function handleKeyDown(event: KeyboardEvent): void {
  const top = topDialog();
  if (!top || event.isComposing) return;
  if (event.key === 'Escape') {
    event.preventDefault();
    event.stopImmediatePropagation();
    top.onClose();
    return;
  }
  if (event.key !== 'Tab') return;
  const elements = focusableElements(top.element);
  const active = document.activeElement;
  const index = elements.findIndex(element => element === active);
  if (!elements.length || index === -1 || (event.shiftKey ? index === 0 : index === elements.length - 1)) {
    event.preventDefault();
    event.stopImmediatePropagation();
    const target = event.shiftKey ? elements[elements.length - 1] : elements[0];
    (target ?? top.element).focus({ preventScroll: true });
  }
}

function handleFocusIn(event: FocusEvent): void {
  const top = topDialog();
  if (!top || !(event.target instanceof HTMLElement)) return;
  if (top.element.contains(event.target)) top.lastFocused = event.target;
  else focusDialog(top);
}

function install(): void {
  bodyOverflow = document.body.style.overflow;
  document.body.style.overflow = 'hidden';
  document.addEventListener('keydown', handleKeyDown, true);
  document.addEventListener('focusin', handleFocusIn, true);
  observer = new MutationObserver(() => {
    syncInert();
    const top = topDialog();
    if (top && !top.element.contains(document.activeElement)) focusDialog(top);
  });
  observer.observe(document.body, { childList: true, subtree: true });
}

function uninstall(): void {
  observer?.disconnect();
  observer = null;
  document.removeEventListener('keydown', handleKeyDown, true);
  document.removeEventListener('focusin', handleFocusIn, true);
  document.body.style.overflow = bodyOverflow;
}

export function useDialogFocus<T extends HTMLElement>(
  dialogRef: RefObject<T>,
  isOpen: boolean,
  onClose: () => void,
): { isTopDialog: () => boolean } {
  const onCloseRef = useRef(onClose);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  onCloseRef.current = onClose;

  useLayoutEffect(() => {
    if (!isOpen) {
      restoreFocusRef.current = null;
      return;
    }
    const element = dialogRef.current;
    if (!element) return;
    const active = document.activeElement;
    if (active instanceof HTMLElement && !element.contains(active)) restoreFocusRef.current = active;
    const entry: DialogEntry = {
      element,
      onClose: () => onCloseRef.current(),
      restoreFocus: restoreFocusRef.current,
      lastFocused: null,
    };
    if (!dialogs.length) install();
    // Child layout effects run first; keep descendants above their parents.
    const childIndex = dialogs.findIndex(dialog => element.contains(dialog.element));
    if (childIndex < 0) dialogs.push(entry);
    else {
      entry.restoreFocus ??= dialogs[childIndex].restoreFocus;
      dialogs.splice(childIndex, 0, entry);
    }
    revision += 1;
    syncInert();
    if (topDialog() === entry) focusDialog(entry);

    return () => {
      const wasTop = topDialog() === entry;
      const index = dialogs.indexOf(entry);
      if (index >= 0) dialogs.splice(index, 1);
      for (const remaining of dialogs) {
        if (remaining.restoreFocus && entry.element.contains(remaining.restoreFocus)) remaining.restoreFocus = entry.restoreFocus;
      }
      const cleanupRevision = ++revision;
      syncInert();
      if (!dialogs.length) uninstall();
      if (!wasTop) return;
      // Let replacement dialogs mount before returning focus to a stale opener.
      queueMicrotask(() => {
        if (cleanupRevision !== revision) return;
        const top = topDialog();
        const target = entry.restoreFocus;
        if (target && isAvailable(target) && (!top || top.element.contains(target))) target.focus({ preventScroll: true });
        else if (top) focusDialog(top);
      });
    };
  }, [dialogRef, isOpen]);

  return { isTopDialog: () => topDialog()?.element === dialogRef.current };
}
