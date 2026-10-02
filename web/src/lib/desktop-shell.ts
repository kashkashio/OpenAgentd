/**
 * Desktop-app chrome for the Tauri shell (macOS, Windows, Linux).
 *
 * A browser tab is a document; the desktop app is an app. Inside Tauri:
 *
 * - ``<html data-shell="desktop">`` turns on the desktop CSS in index.css:
 *   UI chrome does not highlight on drag, buttons use the arrow cursor,
 *   links and images cannot be dragged out.
 * - The webview's own right-click menu (Reload, Back, Inspect, and "Open
 *   Link", which would load the link inside the app) is suppressed. Text
 *   fields and selected text keep the native menu (Copy, Paste, Look Up);
 *   surfaces with an app menu call ``preventDefault`` in their own
 *   ``onContextMenu``, which runs first. Dev builds keep the native menu
 *   for Inspect Element.
 * - ``data-window-inactive`` is set while another app has focus, so the UI
 *   can mute keyboard focus and selections the way native windows do.
 *
 * The browser build keeps the browser's behaviour: none of this runs there.
 */
import { getPlatform } from '@/hooks/use-platform'

const DESKTOP_OS = new Set(['macos', 'windows', 'linux'])

export function isDesktopShell(): boolean {
  const { isTauri, os } = getPlatform()
  return isTauri && DESKTOP_OS.has(os)
}

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false
  if (target instanceof HTMLElement && target.isContentEditable) return true
  return target.closest('input, textarea, select, [contenteditable="true"]') !== null
}

/** The user has selected text: the native menu (Copy, Look Up) wins. */
export function hasTextSelection(): boolean {
  const selection = typeof window.getSelection === 'function' ? window.getSelection() : null
  return Boolean(selection && !selection.isCollapsed && selection.toString().trim())
}

/** Whether a right-click should show the webview's native menu. */
export function allowNativeContextMenu(event: MouseEvent, dev = import.meta.env.DEV): boolean {
  return dev || isEditable(event.target) || hasTextSelection()
}

function onContextMenu(event: MouseEvent): void {
  if (event.defaultPrevented || allowNativeContextMenu(event)) return
  event.preventDefault()
}

function setInactive(inactive: boolean): void {
  if (inactive) document.documentElement.setAttribute('data-window-inactive', '')
  else document.documentElement.removeAttribute('data-window-inactive')
}

// Focus moving into a Preview or MCP app iframe blurs the window too; the
// document still has focus then, so check after the move settles.
function onBlur(): void {
  setTimeout(() => setInactive(!document.hasFocus()), 0)
}

function onFocus(): void {
  setInactive(false)
}

let uninstall: (() => void) | null = null

/** Install once at startup; a no-op outside the desktop app. */
export function installDesktopShell(): () => void {
  if (uninstall) return uninstall
  if (typeof window === 'undefined' || !isDesktopShell()) return () => {}
  const root = document.documentElement
  root.setAttribute('data-shell', 'desktop')
  window.addEventListener('contextmenu', onContextMenu)
  window.addEventListener('blur', onBlur)
  window.addEventListener('focus', onFocus)
  uninstall = () => {
    root.removeAttribute('data-shell')
    setInactive(false)
    window.removeEventListener('contextmenu', onContextMenu)
    window.removeEventListener('blur', onBlur)
    window.removeEventListener('focus', onFocus)
    uninstall = null
  }
  return uninstall
}
