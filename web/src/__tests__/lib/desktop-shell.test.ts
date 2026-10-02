import { afterEach, beforeEach, describe, expect, it } from 'bun:test'

import { allowNativeContextMenu, installDesktopShell, isDesktopShell } from '@/lib/desktop-shell'

const original = { platform: navigator.platform, maxTouchPoints: navigator.maxTouchPoints }

function setPlatform(platform: string, tauri: boolean): void {
  Object.defineProperty(navigator, 'platform', { value: platform, configurable: true, writable: true })
  Object.defineProperty(navigator, 'maxTouchPoints', { value: 0, configurable: true, writable: true })
  const win = window as unknown as { __TAURI_INTERNALS__?: unknown }
  if (tauri) win.__TAURI_INTERNALS__ = {}
  else delete win.__TAURI_INTERNALS__
}

function rightClick(target: Element): MouseEvent {
  const event = new MouseEvent('contextmenu', { bubbles: true, cancelable: true })
  target.dispatchEvent(event)
  return event
}

let uninstall: () => void = () => {}

beforeEach(() => {
  document.body.innerHTML = '<div id="chrome">Sidebar</div><input id="field" /><a id="link" href="https://example.com">x</a>'
})

afterEach(() => {
  uninstall()
  setPlatform(original.platform, false)
  Object.defineProperty(navigator, 'maxTouchPoints', { value: original.maxTouchPoints, configurable: true, writable: true })
  window.getSelection()?.removeAllRanges()
  document.body.innerHTML = ''
})

describe('desktop shell', () => {
  it('applies only inside the desktop Tauri app', () => {
    setPlatform('MacIntel', false)
    expect(isDesktopShell()).toBe(false)
    uninstall = installDesktopShell()
    expect(document.documentElement.hasAttribute('data-shell')).toBe(false)

    setPlatform('MacIntel', true)
    expect(isDesktopShell()).toBe(true)
    uninstall = installDesktopShell()
    expect(document.documentElement.getAttribute('data-shell')).toBe('desktop')
    uninstall()
    expect(document.documentElement.hasAttribute('data-shell')).toBe(false)
  })

  it('suppresses the webview menu on chrome and links but not on fields or selected text', () => {
    setPlatform('Win32', true)
    uninstall = installDesktopShell()
    expect(rightClick(document.getElementById('chrome')!).defaultPrevented).toBe(true)
    expect(rightClick(document.getElementById('link')!).defaultPrevented).toBe(true)
    expect(rightClick(document.getElementById('field')!).defaultPrevented).toBe(false)

    const range = document.createRange()
    range.selectNodeContents(document.getElementById('chrome')!)
    window.getSelection()?.addRange(range)
    expect(rightClick(document.getElementById('chrome')!).defaultPrevented).toBe(false)
  })

  it('keeps the native menu in dev builds', () => {
    const event = new MouseEvent('contextmenu')
    expect(allowNativeContextMenu(event, true)).toBe(true)
    expect(allowNativeContextMenu(event, false)).toBe(false)
  })

  it('marks the window inactive while another app has focus', async () => {
    setPlatform('Linux x86_64', true)
    uninstall = installDesktopShell()
    const hasFocus = document.hasFocus
    document.hasFocus = () => false
    window.dispatchEvent(new Event('blur'))
    await new Promise((resolve) => setTimeout(resolve, 5))
    expect(document.documentElement.hasAttribute('data-window-inactive')).toBe(true)
    window.dispatchEvent(new Event('focus'))
    expect(document.documentElement.hasAttribute('data-window-inactive')).toBe(false)
    document.hasFocus = hasFocus
  })
})
