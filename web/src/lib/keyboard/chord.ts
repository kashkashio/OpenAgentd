/**
 * Key chords: what a shortcut listens for, matched against a key press.
 *
 * ``mod`` is the platform's primary modifier (⌘ on macOS, Ctrl elsewhere,
 * see ``keyboard-shortcut.ts``); the other one must be up so a stray ⌘+Ctrl
 * never fires. Shift and Alt must match exactly, except that Shift is
 * ignored for a printed symbol whose character already carries it ("+" is
 * Shift+= on US layouts). ``code`` matches the physical key instead, for
 * chords whose character changes with the layout or with Alt (⌥C is "ç").
 * ``ctrl`` is the Control key itself on every OS (⌃Tab), for the few
 * chords that are Control even on macOS; it cannot combine with ``mod``.
 */
import type { OS } from '@/hooks/use-platform'
import { isPrimaryModifierOS } from '@/lib/keyboard-shortcut'

export interface KeyChord {
  key: string
  code?: string
  mod?: boolean
  ctrl?: boolean
  shift?: boolean
  alt?: boolean
}

function normalizeKey(key: string): string {
  if (key === 'Esc') return 'Escape'
  return key.length === 1 ? key.toLowerCase() : key
}

function isPrintedSymbol(chord: KeyChord): boolean {
  return !chord.code && chord.key.length === 1 && !/[a-z0-9]/i.test(chord.key)
}

export function matchChord(event: KeyboardEvent, chord: KeyChord, os: OS): boolean {
  if (chord.ctrl) {
    if (!event.ctrlKey || event.metaKey) return false
  } else {
    const mac = isPrimaryModifierOS(os)
    const primary = mac ? event.metaKey : event.ctrlKey
    const other = mac ? event.ctrlKey : event.metaKey
    if (other || primary !== Boolean(chord.mod)) return false
  }
  if (event.altKey !== Boolean(chord.alt)) return false
  const shiftFree = chord.shift === undefined && isPrintedSymbol(chord)
  if (!shiftFree && event.shiftKey !== Boolean(chord.shift)) return false
  if (chord.code) return event.code === chord.code
  return normalizeKey(event.key) === normalizeKey(chord.key)
}

/** True while an input method is still composing (the key belongs to it). */
export function isImeComposing(event: KeyboardEvent): boolean {
  return event.isComposing || event.keyCode === 229
}

/** Stable identity for a chord, e.g. to re-register when it changes. */
export function chordId(chord: KeyChord): string {
  return [chord.mod && 'mod', chord.ctrl && 'ctrl', chord.alt && 'alt', chord.shift && 'shift', chord.code ?? normalizeKey(chord.key)]
    .filter(Boolean)
    .join('+')
}

const ARROWS: Record<string, string> = { ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→' }

function keyLabel(key: string, mac: boolean): string {
  if (ARROWS[key]) return ARROWS[key]
  if (key === 'Escape' || key === 'Esc') return 'Esc'
  if (key === 'Enter') return mac ? '↵' : 'Enter'
  if (key === 'Backspace') return mac ? '⌫' : 'Backspace'
  if (key === 'Delete') return mac ? '⌦' : 'Delete'
  return key.length === 1 ? key.toUpperCase() : key
}

/** Human-readable label for any chord, e.g. ``⌥⌘↑``, ``Ctrl+Shift+D``, ``Esc``. */
export function formatChord(chord: KeyChord, os: OS): string {
  const mac = isPrimaryModifierOS(os)
  const key = keyLabel(chord.key, mac)
  if (mac) return `${chord.ctrl ? '⌃' : ''}${chord.alt ? '⌥' : ''}${chord.mod ? '⌘' : ''}${chord.shift ? '⇧' : ''}${key}`
  return [(chord.mod || chord.ctrl) && 'Ctrl', chord.alt && 'Alt', chord.shift && 'Shift', key].filter(Boolean).join('+')
}
