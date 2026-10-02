/**
 * Keys a focused terminal gives back to the app.
 *
 * App shortcuts listen on ``window`` and skip keys a focused widget handled.
 * xterm handles (and stops) every key it sends to the shell, but it ignores
 * ⌘ chords, so on macOS every app shortcut already works from a terminal:
 * ⌘K opens the palette and ⌘F the find bar. Clearing the scrollback is in
 * the terminal tab's menu instead.
 *
 * Elsewhere the app's primary modifier is Ctrl, which is also the shell's.
 * Ctrl chords stay with the shell (Ctrl+C, Ctrl+D, Ctrl+W, Ctrl+R…), except
 * Ctrl+K and Ctrl+P: like VS Code, they open the command palette and quick
 * open rather than kill-line and previous-command.
 */
import type { OS } from '@/hooks/use-platform'
import { isPrimaryModifierOS } from '@/lib/keyboard-shortcut'

/** Ctrl chords that skip the shell on Windows/Linux. */
const APP_KEYS_OVER_SHELL = new Set(['k', 'p'])

/**
 * xterm custom key handler body: returns ``false`` when xterm must not
 * process the key. The event is left unclaimed so the app shortcut runs.
 */
export function routeTerminalKey(event: KeyboardEvent, os: OS): boolean {
  if (event.type !== 'keydown' || isPrimaryModifierOS(os)) return true
  if (!event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return true
  return !APP_KEYS_OVER_SHELL.has(event.key.toLowerCase())
}
