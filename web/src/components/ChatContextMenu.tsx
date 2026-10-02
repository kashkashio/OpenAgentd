/**
 * App right-click menus for chat content: links, code blocks, messages.
 *
 * In the desktop app the webview's own menu (Reload, Inspect, "Open Link"
 * inside the app) is suppressed everywhere (``lib/desktop-shell.ts``), so a
 * transcript surface offers its own: Open / Copy link, Copy code, Copy
 * response… Browsers keep their native menu on right-click; Shift+F10 or
 * the ContextMenu key opens the app menu on every platform. Selected text
 * always gets the native menu, which has Copy.
 *
 * The innermost surface wins: a link inside a message opens the link menu,
 * and the message's handler sees the event already handled.
 */
import { useState, type KeyboardEvent, type MouseEvent, type ReactNode, type SyntheticEvent } from 'react'
import { createPortal } from 'react-dom'

import { CONTEXT_MENU_ITEM_CLASS, ContextMenu } from '@/components/ui/context-menu'
import { hasTextSelection, isDesktopShell } from '@/lib/desktop-shell'
import { isMenuKey, menuPointFor } from '@/lib/focus/item-keys'

export interface ChatMenuItem {
  label: string
  run: () => void
}

function coarsePointer(): boolean {
  return typeof window.matchMedia === 'function' && window.matchMedia('(pointer: coarse)').matches
}

// The menu is portalled, but React events still bubble through the tree:
// keep its clicks and keys from reaching the link or message that opened it.
const stop = (event: SyntheticEvent) => event.stopPropagation()

/** Handlers for a surface plus the menu to render; ``items`` is read on open. */
export function useChatMenu(label: string, items: () => ChatMenuItem[]) {
  const [open, setOpen] = useState<{ x: number; y: number; items: ChatMenuItem[] } | null>(null)
  const show = (x: number, y: number) => {
    const list = items()
    if (list.length > 0) setOpen({ x, y, items: list })
  }
  const onContextMenu = (event: MouseEvent<HTMLElement>) => {
    if (event.isDefaultPrevented() || !isDesktopShell() || coarsePointer() || hasTextSelection()) return
    event.preventDefault()
    show(event.clientX, event.clientY)
  }
  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.isDefaultPrevented() || !isMenuKey(event) || coarsePointer()) return
    event.preventDefault()
    const at = menuPointFor(event.target instanceof HTMLElement ? event.target : event.currentTarget)
    show(at.clientX, at.clientY)
  }
  const menu: ReactNode = open
    ? createPortal(
        <div className="contents" onClick={stop} onContextMenu={stop} onKeyDown={stop} onPointerDown={stop}>
          <ContextMenu at={open} label={label} onDismiss={() => setOpen(null)}>
            {open.items.map((item) => (
              <button
                key={item.label}
                type="button"
                role="menuitem"
                className={CONTEXT_MENU_ITEM_CLASS}
                onClick={() => {
                  setOpen(null)
                  item.run()
                }}
              >
                {item.label}
              </button>
            ))}
          </ContextMenu>
        </div>,
        document.body,
      )
    : null
  return { onContextMenu, onKeyDown, menu }
}

export function copyText(text: string): void {
  void navigator.clipboard?.writeText(text).catch(() => {})
}
