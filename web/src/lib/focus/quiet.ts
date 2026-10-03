const QUIET = 'data-quiet-focus'

/**
 * Focus without the keyboard ring, for focus the user did not steer by
 * keyboard: a click (WebKit counts the script ``focus()`` that gives a
 * clicked button focus as keyboard focus and draws the ring), a tab switch
 * that moves focus onto the new tab, or focus handed back on page load.
 * index.css drops the outline while ``data-quiet-focus`` is set; it clears
 * on blur, so the next control reached with the keyboard shows its ring.
 */
export function focusQuietly(el: HTMLElement, options: FocusOptions = { preventScroll: true }): void {
  if (!el.hasAttribute(QUIET)) {
    el.setAttribute(QUIET, '')
    el.addEventListener('blur', () => el.removeAttribute(QUIET), { once: true })
  }
  if (document.activeElement !== el) el.focus(options)
  if (document.activeElement !== el) el.removeAttribute(QUIET)
}
