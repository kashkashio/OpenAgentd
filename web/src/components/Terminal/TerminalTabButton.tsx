/**
 * TerminalTabButton — tab chip for a terminal session in
 * WorkspacePanel (terminal needs an attached workspace).
 *
 * Desktop: right-click opens a small menu (Rename / Clear / Close).
 * Mobile: long-press opens the same choice as a bottom sheet — no native
 * context menu on touch, matching the LongPressButton pattern used
 * elsewhere (Sidebar sessions, changed-files, commits).
 * Both funnel into useTerminalStore.rename() / .clear() / .close().
 */

import { useRef, useState, type ReactNode } from 'react'
import { Eraser, Pencil, TerminalSquare, X } from 'lucide-react'

import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import {
  CONTEXT_MENU_ITEM_CLASS,
  CONTEXT_MENU_ITEM_DANGER_CLASS,
  ContextMenu,
  ContextMenuSeparator,
} from '@/components/ui/context-menu'
import { Input } from '@/components/ui/input'
import { LongPressButton } from '@/components/ui/long-press-button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import {
  dockTabButtonClass,
  dockTabClass,
  dockTabCloseClass,
} from '@/components/WorkspacePanel/dock-tab-styles'
import { softHapticFeedback } from '@/lib/haptics'
import { isMenuKey, menuPointFor } from '@/lib/focus/item-keys'
import { cn } from '@/lib/utils'
import { useTerminalStore, type TerminalSessionMeta } from '@/stores/useTerminalStore'

interface TerminalTabButtonProps {
  meta: TerminalSessionMeta
  active: boolean
  mobile: boolean
  onActivate: () => void
  className?: string
  buttonRef?: (node: HTMLButtonElement | null) => void
  /** Tab-strip items (Close Others, …) appended to the desktop menu. */
  extraMenuItems?: (dismiss: () => void) => ReactNode
  /** Closes the tab through the dock, which picks the next active tab. */
  onClose?: () => void
  /** The dock strip's tab id, marking the wrapper as a drag handle. */
  dockTabId?: string
}

export function TerminalTabButton({
  meta,
  active,
  mobile,
  onActivate,
  className,
  buttonRef,
  extraMenuItems,
  onClose,
  dockTabId,
}: TerminalTabButtonProps) {
  const [desktopMenuAt, setDesktopMenuAt] = useState<{ x: number; y: number } | null>(null)
  const [mobileSheetOpen, setMobileSheetOpen] = useState(false)
  const [renaming, setRenaming] = useState(false)
  const [draftTitle, setDraftTitle] = useState(meta.title)
  const renameInputRef = useRef<HTMLInputElement>(null)
  const close = onClose ?? (() => useTerminalStore.getState().close(meta.id))

  const openRename = () => {
    setDraftTitle(meta.title)
    setRenaming(true)
  }

  const submitRename = (e: React.FormEvent) => {
    e.preventDefault()
    useTerminalStore.getState().rename(meta.id, draftTitle)
    setRenaming(false)
  }

  return (
    <>
      {/* Same editor-tab chrome as the dock's file/diff/commit tabs: the
          wrapper carries the tab surface, the activate button and the close
          button are siblings (never a control nested inside a button). */}
      <div data-dock-tab={dockTabId} className={cn(dockTabClass(active), className)}>
        <Tooltip className="h-full min-w-0 flex-1">
          <TooltipTrigger
            className="h-full min-w-0 flex-1"
            render={
              <LongPressButton
                ref={buttonRef}
                type="button"
                aria-current={active ? 'true' : undefined}
                enabled={mobile}
                onLongPress={() => {
                  softHapticFeedback()
                  setMobileSheetOpen(true)
                }}
                onContextMenu={(e) => {
                  if (mobile) return
                  e.preventDefault()
                  setDesktopMenuAt({ x: e.clientX, y: e.clientY })
                }}
                onKeyDown={(e) => {
                  if (mobile || !isMenuKey(e)) return
                  e.preventDefault()
                  const at = menuPointFor(e.currentTarget)
                  setDesktopMenuAt({ x: at.clientX, y: at.clientY })
                }}
                onClick={onActivate}
                onAuxClick={(e) => {
                  if (mobile || e.button !== 1) return
                  e.preventDefault()
                  close()
                }}
                className={cn(dockTabButtonClass(!mobile), 'flex-1')}
              >
                <TerminalSquare size={12} className="shrink-0" aria-hidden="true" />
                <span className="truncate font-mono">{meta.title}</span>
              </LongPressButton>
            }
          />
          <TooltipContent>{meta.title}</TooltipContent>
        </Tooltip>
        {!mobile && (
          <button
            type="button"
            data-zone-skip
            onClick={(e) => {
              e.stopPropagation()
              close()
            }}
            className={dockTabCloseClass(active)}
            aria-label={`Close ${meta.title}`}
          >
            <X size={11} aria-hidden="true" />
          </button>
        )}
      </div>

      {/* Desktop: right-click menu */}
      {desktopMenuAt && (
        <ContextMenu at={desktopMenuAt} label={`Actions for ${meta.title}`} onDismiss={() => setDesktopMenuAt(null)}>
          <button
            type="button"
            role="menuitem"
            className={CONTEXT_MENU_ITEM_CLASS}
            onClick={() => { setDesktopMenuAt(null); openRename() }}
          >
            <Pencil size={12} aria-hidden="true" />
            Rename
          </button>
          <button
            type="button"
            role="menuitem"
            className={CONTEXT_MENU_ITEM_CLASS}
            onClick={() => {
              setDesktopMenuAt(null)
              useTerminalStore.getState().clear(meta.id)
            }}
          >
            <Eraser size={12} aria-hidden="true" />
            Clear
          </button>
          <button
            type="button"
            role="menuitem"
            className={CONTEXT_MENU_ITEM_DANGER_CLASS}
            onClick={() => {
              setDesktopMenuAt(null)
              close()
            }}
          >
            <X size={12} aria-hidden="true" />
            Close
          </button>
          {extraMenuItems && (
            <>
              <ContextMenuSeparator />
              {extraMenuItems(() => setDesktopMenuAt(null))}
            </>
          )}
        </ContextMenu>
      )}

      {/* Mobile: long-press action sheet */}
      <Dialog open={mobileSheetOpen} onOpenChange={setMobileSheetOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle className="truncate">{meta.title}</DialogTitle>
            <DialogDescription>Choose a terminal action.</DialogDescription>
          </DialogHeader>
          <DialogFooter className="flex-col items-stretch gap-2 p-3 sm:flex-col">
            <Button
              type="button"
              variant="ghost"
              className="justify-start"
              onClick={() => { setMobileSheetOpen(false); openRename() }}
            >
              <Pencil size={14} aria-hidden="true" />
              Rename
            </Button>
            <Button
              type="button"
              variant="ghost"
              className="justify-start"
              onClick={() => {
                setMobileSheetOpen(false)
                useTerminalStore.getState().clear(meta.id)
              }}
            >
              <Eraser size={14} aria-hidden="true" />
              Clear terminal
            </Button>
            <Button
              type="button"
              variant="danger-subtle"
              className="justify-start"
              onClick={() => {
                setMobileSheetOpen(false)
                close()
              }}
              aria-label="Close terminal"
            >
              <X size={14} aria-hidden="true" />
              Close terminal
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Shared rename dialog (desktop menu + mobile sheet both open it) */}
      <Dialog
        open={renaming}
        onOpenChange={(open) => {
          setRenaming(open)
          if (open) window.setTimeout(() => renameInputRef.current?.select(), 0)
        }}
      >
        <DialogContent showCloseButton={false}>
          <form onSubmit={submitRename}>
            <DialogHeader>
              <DialogTitle>Rename terminal</DialogTitle>
              <DialogDescription>Give this session a memorable name.</DialogDescription>
            </DialogHeader>
            <div className="px-3 py-2">
              <Input
                ref={renameInputRef}
                value={draftTitle}
                onChange={(e) => setDraftTitle(e.target.value)}
                aria-label="Terminal name"
                maxLength={64}
                autoFocus
              />
            </div>
            <DialogFooter className="p-3">
              <Button type="button" variant="default" onClick={() => setRenaming(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={!draftTitle.trim()}>
                Save
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </>
  )
}
