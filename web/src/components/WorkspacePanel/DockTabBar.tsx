/**
 * DockTabBar — the review dock's editor-tab strip.
 *
 * One row: scrolling tabs on the left, a fixed action cluster on the right
 * (new preview, new terminal, refresh, and on desktop maximize; file search is Quick
 * Open, ⌘P). Hiding the
 * dock lives on the header's review-dock toggle (and ⌘D), which is always
 * visible because the dock never covers the header.
 *
 * Tabs are plain buttons with ``aria-current`` rather than an ARIA
 * ``tablist`` (each has a sibling close button and a context menu, which
 * tab semantics do not model), but the whole strip is one focus zone: a
 * single Tab stop, entered on the active tab, with Left/Right across tabs
 * and actions. Close buttons are out of Tab order; ⌘W, middle-click and
 * the tab menu (right-click or Shift+F10) close tabs from the keyboard.
 */
import { useRef, useState, type ReactNode } from 'react'
import { CalendarClock, Copy, FileDiff, FileText, GitCommitHorizontal, GitCompare, Globe, ListTodo, Maximize2, Minimize2, RefreshCw, TerminalSquare, X } from 'lucide-react'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { CONTEXT_MENU_ITEM_CLASS, ContextMenu } from '@/components/ui/context-menu'
import { FileTypeIcon } from '../FileTypeIcon'
import { TerminalTabButton } from '../Terminal/TerminalTabButton'
import type { OS } from '@/hooks/use-platform'
import { APP_SHORTCUTS, shortcutLabel } from '@/lib/app-shortcuts'
import { useFocusZone } from '@/lib/focus/zones'
import { isMenuKey, menuPointFor } from '@/lib/focus/item-keys'
import { cn } from '@/lib/utils'
import type { TerminalSessionMeta } from '@/stores/useTerminalStore'
import {
  DOCK_ACTION_BUTTON_CLASS,
  dockTabButtonClass,
  dockTabClass,
  dockTabCloseClass,
} from './dock-tab-styles'
import { type DockTab, dockTabLabel, dockTabTooltip, isViewTab } from './dock-tabs'


export interface DockTabBarProps {
  tabs: DockTab[]
  activeTabId: string
  workspace: string
  terminalMetas: TerminalSessionMeta[]
  mobile: boolean
  os: OS
  registerTabRef: (id: string, node: HTMLButtonElement | null) => void
  onActivate: (id: string) => void
  onClose: (id: string) => void
  /** Tab menu: close every other tab / the tabs after this one. */
  onCloseOthers?: (id: string) => void
  onCloseToRight?: (id: string) => void
  onNewTerminal: () => void
  /** Opens a web preview tab; omitted when previews are unavailable. */
  onNewPreview?: () => void
  onRefresh: () => void
  /** ``null`` hides the toggle (mobile, or a forced narrow-window overlay). */
  maximized: boolean | null
  onToggleMaximized: () => void
}

function TabIcon({ tab }: { tab: DockTab }) {
  switch (tab.type) {
    case 'review':
      return <GitCompare size={12} className="shrink-0" aria-hidden="true" />
    case 'tasks':
      return <ListTodo size={12} className="shrink-0" aria-hidden="true" />
    case 'schedule':
      return <CalendarClock size={12} className="shrink-0" aria-hidden="true" />
    case 'plan':
      return <FileText size={12} className="shrink-0" aria-hidden="true" />
    case 'file':
      return <FileTypeIcon name={tab.file.name || tab.file.path} size={13} />
    case 'diff':
      return <FileDiff size={12} className="shrink-0 text-(--color-text-subtle)" aria-hidden="true" />
    case 'commit':
      return <GitCommitHorizontal size={12} className="shrink-0 text-(--color-text-subtle)" aria-hidden="true" />
    case 'preview':
      return <Globe size={12} className="shrink-0 text-(--color-text-subtle)" aria-hidden="true" />
    default:
      return null
  }
}

function ActionButton({ label, onClick, children }: { label: string; onClick?: () => void; children: React.ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <button type="button" onClick={onClick} className={DOCK_ACTION_BUTTON_CLASS} aria-label={label}>
            {children}
          </button>
        }
      />
      <TooltipContent side="bottom">{label}</TooltipContent>
    </Tooltip>
  )
}

/** The path a file or diff tab shows, absolute, for "Copy Path". */
function tabPath(tab: DockTab, workspace: string): string | null {
  const path = tab.type === 'file' ? tab.file.path : tab.type === 'diff' ? tab.path : null
  if (!path) return null
  return path.startsWith('/') ? path : `${workspace.replace(/\/+$/, '')}/${path}`
}

/**
 * The tab-strip items every tab's menu shares (terminal tabs add theirs
 * first). ``dismiss`` closes the menu before the action runs.
 */
export function DockTabMenuItems({ tab, tabs, workspace, dismiss, onClose, onCloseOthers, onCloseToRight }: {
  tab: DockTab
  tabs: DockTab[]
  workspace: string
  dismiss: () => void
  onClose?: (id: string) => void
  onCloseOthers?: (id: string) => void
  onCloseToRight?: (id: string) => void
}): ReactNode {
  const index = tabs.findIndex((item) => item.id === tab.id)
  const hasOthers = tabs.some((item) => item.id !== tab.id && item.type !== 'review')
  const hasRight = tabs.slice(index + 1).some((item) => item.type !== 'review')
  const path = tabPath(tab, workspace)
  const item = (label: string, run: () => void, icon?: ReactNode, disabled = false) => (
    <button
      type="button"
      role="menuitem"
      disabled={disabled}
      className={cn(CONTEXT_MENU_ITEM_CLASS, 'disabled:opacity-50')}
      onClick={() => { dismiss(); run() }}
    >
      {icon ?? <span className="w-3" aria-hidden="true" />}
      {label}
    </button>
  )
  return (
    <>
      {onClose && tab.type !== 'review' && item('Close', () => onClose(tab.id), <X size={12} aria-hidden="true" />)}
      {onCloseOthers && item('Close Others', () => onCloseOthers(tab.id), undefined, !hasOthers)}
      {onCloseToRight && item('Close to the Right', () => onCloseToRight(tab.id), undefined, !hasRight)}
      {path && item('Copy Path', () => { void navigator.clipboard?.writeText(path) }, <Copy size={12} aria-hidden="true" />)}
    </>
  )
}

export function DockTabBar({
  tabs,
  activeTabId,
  workspace,
  terminalMetas,
  mobile,
  os,
  registerTabRef,
  onActivate,
  onClose,
  onCloseOthers,
  onCloseToRight,
  onNewTerminal,
  onNewPreview,
  onRefresh,
  maximized,
  onToggleMaximized,
}: DockTabBarProps) {
  const stripRef = useRef<HTMLDivElement>(null)
  useFocusZone(stripRef, { orientation: 'horizontal', entry: 'active', wrap: true })
  const [menu, setMenu] = useState<{ id: string; x: number; y: number } | null>(null)
  const menuTab = menu ? tabs.find((tab) => tab.id === menu.id) : undefined
  const sharedMenu = (tab: DockTab, dismiss: () => void) => (
    <DockTabMenuItems
      tab={tab}
      tabs={tabs}
      workspace={workspace}
      dismiss={dismiss}
      onCloseOthers={onCloseOthers}
      onCloseToRight={onCloseToRight}
    />
  )
  return (
    <div ref={stripRef} className="flex h-(--spacing-tab-bar) min-w-0 shrink-0 bg-(--bg-sidebar)">
      <div className="scrollbar-none flex min-w-0 flex-1 overflow-x-auto overflow-y-hidden">
        {tabs.map((tab) => {
          const active = activeTabId === tab.id
          if (tab.type === 'terminal') {
            return (
              <TerminalTabButton
                key={tab.id}
                buttonRef={(node) => registerTabRef(tab.id, node)}
                meta={terminalMetas.find((m) => m.id === tab.termId) ?? {
                  id: tab.termId, contextKey: workspace, title: tab.title, status: 'connecting', order: 0,
                }}
                active={active}
                mobile={mobile}
                onActivate={() => onActivate(tab.id)}
                extraMenuItems={(dismiss) => sharedMenu(tab, dismiss)}
              />
            )
          }
          const closable = tab.type !== 'review'
          const label = dockTabLabel(tab)
          const tooltip = dockTabTooltip(tab)
          const tabButton = (
            <button
              ref={(node) => registerTabRef(tab.id, node)}
              type="button"
              aria-current={active ? 'true' : undefined}
              aria-label={label === tab.title ? undefined : label}
              onClick={() => onActivate(tab.id)}
              onContextMenu={(event) => {
                if (mobile) return
                event.preventDefault()
                setMenu({ id: tab.id, x: event.clientX, y: event.clientY })
              }}
              onKeyDown={(event) => {
                if (mobile || !isMenuKey(event)) return
                event.preventDefault()
                const at = menuPointFor(event.currentTarget)
                setMenu({ id: tab.id, x: at.clientX, y: at.clientY })
              }}
              onAuxClick={(event) => {
                if (!closable || event.button !== 1) return
                event.preventDefault()
                onClose(tab.id)
              }}
              className={cn(dockTabButtonClass(closable), 'flex-1')}
            >
              <TabIcon tab={tab} />
              <span className={cn('truncate', !isViewTab(tab) && 'font-mono')}>{tab.title}</span>
            </button>
          )
          return (
            <div key={tab.id} className={dockTabClass(active)}>
              {tooltip ? (
                <Tooltip className="h-full min-w-0 flex-1">
                  <TooltipTrigger className="h-full min-w-0 flex-1" render={tabButton} />
                  <TooltipContent side="bottom">{tooltip}</TooltipContent>
                </Tooltip>
              ) : tabButton}
              {closable && (
                <button
                  type="button"
                  data-zone-skip
                  onClick={() => onClose(tab.id)}
                  className={dockTabCloseClass(active)}
                  aria-label={`Close ${label}`}
                >
                  <X size={11} aria-hidden="true" />
                </button>
              )}
            </div>
          )
        })}
        <div aria-hidden="true" className="min-w-2 flex-1 border-b border-(--color-border)" />
      </div>
      <div className="flex shrink-0 items-center gap-0.5 border-b border-(--color-border) px-1">
        {onNewPreview && (
          <ActionButton label="New preview" onClick={onNewPreview}>
            <Globe size={14} aria-hidden="true" />
          </ActionButton>
        )}
        <ActionButton label="New terminal" onClick={onNewTerminal}>
          <TerminalSquare size={14} aria-hidden="true" />
        </ActionButton>
        <ActionButton label="Refresh" onClick={onRefresh}>
          <RefreshCw size={14} aria-hidden="true" />
        </ActionButton>
        {!mobile && maximized !== null && (
          <ActionButton
            label={`${maximized ? 'Restore' : 'Maximize'} review dock (${shortcutLabel(APP_SHORTCUTS.maximizeDock, os)})`}
            onClick={onToggleMaximized}
          >
            {maximized
              ? <Minimize2 size={13} aria-hidden="true" />
              : <Maximize2 size={13} aria-hidden="true" />}
          </ActionButton>
        )}
      </div>
      {menu && menuTab && (
        <ContextMenu at={menu} label={`Actions for ${dockTabLabel(menuTab)}`} onDismiss={() => setMenu(null)} className="min-w-44">
          <DockTabMenuItems
            tab={menuTab}
            tabs={tabs}
            workspace={workspace}
            dismiss={() => setMenu(null)}
            onClose={onClose}
            onCloseOthers={onCloseOthers}
            onCloseToRight={onCloseToRight}
          />
        </ContextMenu>
      )}
    </div>
  )
}
