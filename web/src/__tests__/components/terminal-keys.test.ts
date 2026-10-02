import { describe, expect, it } from 'bun:test'

import { routeTerminalKey } from '@/components/Terminal/terminal-keys'

function keydown(key: string, mods: Partial<Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey'>> = {}) {
  return new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...mods })
}

describe('routeTerminalKey — macOS', () => {
  it('leaves every key to xterm, which passes ⌘ chords on to the app shortcuts', () => {
    for (const event of [keydown('k', { metaKey: true }), keydown('f', { metaKey: true }), keydown('k', { ctrlKey: true })]) {
      expect(routeTerminalKey(event, 'macos')).toBe(true)
      expect(event.defaultPrevented).toBe(false)
    }
  })
})

describe('routeTerminalKey — Windows/Linux', () => {
  it('keeps Ctrl+K and Ctrl+P from the shell so the palette and quick open still work', () => {
    for (const key of ['k', 'K', 'p']) {
      const event = keydown(key, { ctrlKey: true })
      expect(routeTerminalKey(event, 'linux')).toBe(false)
      // Not claimed here: the app's shortcut listener must still see it.
      expect(event.defaultPrevented).toBe(false)
    }
    expect(routeTerminalKey(keydown('k', { ctrlKey: true }), 'windows')).toBe(false)
  })

  it('sends every other Ctrl chord to the shell', () => {
    for (const event of [keydown('c', { ctrlKey: true }), keydown('d', { ctrlKey: true }), keydown('w', { ctrlKey: true }), keydown('r', { ctrlKey: true })]) {
      expect(routeTerminalKey(event, 'linux')).toBe(true)
    }
    // Modified variants stay with the shell too.
    expect(routeTerminalKey(keydown('k', { ctrlKey: true, shiftKey: true }), 'linux')).toBe(true)
    expect(routeTerminalKey(keydown('p', { ctrlKey: true, altKey: true }), 'linux')).toBe(true)
  })
})

it('ignores keyup and keypress', () => {
  expect(routeTerminalKey(new KeyboardEvent('keyup', { key: 'k', ctrlKey: true }), 'linux')).toBe(true)
})
