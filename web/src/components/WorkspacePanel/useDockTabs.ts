/**
 * Tab state for the review dock's editor strip.
 *
 * Owns which tabs are open and which is active; keeps terminal tabs in step
 * with the terminal store; resets to the workspace's defaults when the dock is
 * handed another workspace; handles the shell's "open terminal" and "open
 * Tasks / Schedule" requests; and closes the active tab on Mod+W. Opening
 * tabs that need query data (changed files, commits by sha) stays with the
 * dock, which owns those queries.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useShallow } from 'zustand/react/shallow'

import type { GitCommit, WorkspaceFileInfo } from '@/api/types'
import type { PreviewTarget } from '@/api/preview'
import { useAppShortcut } from '@/lib/keyboard/hooks'
import { useTerminalStore } from '@/stores/useTerminalStore'

import type { ChangedFileInfo } from './diff-helpers'
import {
  type DiffTabRequest,
  type DockTab,
  type DockView,
  type DockViewRequest,
  type PreviewTabRequest,
  REVIEW_TAB,
  REVIEW_TAB_ID,
  basename,
  commitTabId,
  diffTabId,
  fileTabId,
  previewTabId,
  previewTabTitle,
  terminalIdFromTabId,
  terminalTabId,
  viewTab,
} from './dock-tabs'

/** Insert a tab before the terminal group so terminals stay at the end. */
function withTab(current: DockTab[], tab: DockTab): DockTab[] {
  const index = current.findIndex((item) => item.id === tab.id)
  if (index >= 0) return current.map((item, i) => (i === index ? tab : item))
  const firstTerminal = current.findIndex((item) => item.type === 'terminal')
  if (tab.type === 'terminal' || firstTerminal < 0) return [...current, tab]
  return [...current.slice(0, firstTerminal), tab, ...current.slice(firstTerminal)]
}

interface DockTabsOptions {
  workspace: string
  /** False while the dock is closed (kept mounted, hidden): its keys are off. */
  open?: boolean
  chatWorkspace: boolean
  onFileSelect?: (file: WorkspaceFileInfo | null) => void
  terminalOpenKey: number
  handledTerminalOpenKeyRef?: React.RefObject<number | null>
  viewRequest: DockViewRequest | null
  handledViewRequestKeyRef?: React.RefObject<number>
  onActiveViewChange?: (view: DockView | null) => void
  diffRequest?: DiffTabRequest | null
  handledDiffRequestKeyRef?: React.RefObject<number>
  previewRequest?: PreviewTabRequest | null
  handledPreviewRequestKeyRef?: React.RefObject<number>
  /** Called after a tab leaves the strip (close button, middle-click, Mod+W). */
  onTabClosed?: (tab: DockTab) => void
}

export function useDockTabs({
  workspace,
  open = true,
  chatWorkspace,
  onFileSelect,
  terminalOpenKey,
  handledTerminalOpenKeyRef: parentHandledTerminalOpenKeyRef,
  viewRequest,
  handledViewRequestKeyRef: parentHandledViewRequestKeyRef,
  onActiveViewChange,
  diffRequest,
  handledDiffRequestKeyRef: parentHandledDiffRequestKeyRef,
  previewRequest,
  handledPreviewRequestKeyRef: parentHandledPreviewRequestKeyRef,
  onTabClosed,
}: DockTabsOptions) {
  // Chat workspaces have no Git review tab — the root is not a repository.
  const defaultTabId = chatWorkspace ? '' : REVIEW_TAB_ID
  const [tabs, setTabs] = useState<DockTab[]>(chatWorkspace ? [] : [REVIEW_TAB])
  // A panel mounted for a project workspace can be re-used for a chat one, so
  // filter the review tab out of the strip rather than only skipping it at
  // construction time.
  const visibleTabs = chatWorkspace ? tabs.filter((tab) => tab.type !== 'review') : tabs
  const [activeTabId, setActiveTabId] = useState(defaultTabId)
  // The dock stays open across workspace switches, so this instance can be
  // handed another workspace. Tabs belong to the workspace they were opened
  // in: start over with the new one's defaults (the Git tab when leaving
  // Chat, and no file tabs pointing into the old tree). Terminal tabs are
  // re-derived from the new workspace's sessions by the sync effect below.
  const [tabsWorkspace, setTabsWorkspace] = useState(workspace)
  if (tabsWorkspace !== workspace) {
    setTabsWorkspace(workspace)
    setTabs(chatWorkspace ? [] : [REVIEW_TAB])
    setActiveTabId(defaultTabId)
  }

  const terminalMetas = useTerminalStore(
    useShallow((s) =>
      Object.values(s.sessions)
        .filter((meta) => meta.contextKey === workspace)
        .sort((a, b) => a.order - b.order),
    ),
  )

  const activeTab = useMemo<DockTab | undefined>(() => {
    const found = tabs.find((item) => item.id === activeTabId)
    if (found) return found
    const termId = terminalIdFromTabId(activeTabId)
    const meta = termId ? terminalMetas.find((m) => m.id === termId) : undefined
    if (meta) return { id: activeTabId, type: 'terminal', title: meta.title, termId: meta.id }
    return tabs[0]
  }, [tabs, activeTabId, terminalMetas])

  const openTab = useCallback((tab: DockTab) => {
    setTabs((current) => withTab(current, tab))
    setActiveTabId(tab.id)
  }, [])

  const openFileTab = useCallback((file: WorkspaceFileInfo) => {
    openTab({ id: fileTabId(file.path), type: 'file', title: file.name || basename(file.path), file })
    onFileSelect?.(file)
  }, [openTab, onFileSelect])

  const openDiffTab = useCallback((file: ChangedFileInfo) => {
    openTab({ id: diffTabId(file.path), type: 'diff', title: basename(file.path), path: file.path, status: file.status })
  }, [openTab])

  const openCommitTab = useCallback((commit: GitCommit) => {
    openTab({ id: commitTabId(commit.sha), type: 'commit', title: commit.short_sha, commit })
  }, [openTab])

  /** Open or focus the preview for ``target``; an open tab navigates to it. */
  const openPreviewTab = useCallback((target: PreviewTarget, options?: { focusOnly?: boolean }) => {
    const id = previewTabId(target)
    setTabs((current) => {
      const existing = current.find((item) => item.id === id)
      if (existing && options?.focusOnly) return current
      const navKey = existing?.type === 'preview' ? existing.navKey + 1 : 0
      return withTab(current, { id, type: 'preview', title: previewTabTitle(target), target, navKey })
    })
    setActiveTabId(id)
  }, [])

  useEffect(() => {
    setTabs((current) => {
      const nonTerminal = current.filter((item) => item.type !== 'terminal')
      const terminalTabs = terminalMetas.map((meta) => {
        const existing = current.find(
          (item) => item.type === 'terminal' && item.termId === meta.id,
        )
        return existing && existing.title === meta.title
          ? existing
          : { id: terminalTabId(meta.id), type: 'terminal' as const, title: meta.title, termId: meta.id }
      })
      const changed =
        current.length !== nonTerminal.length + terminalTabs.length ||
        terminalTabs.some((tab) => !current.includes(tab))
      return changed ? [...nonTerminal, ...terminalTabs] : current
    })
  }, [terminalMetas])

  const openTerminal = useCallback(() => {
    const id = useTerminalStore.getState().open({ workspace }, workspace)
    const meta = useTerminalStore.getState().sessionsForContext(workspace).find((m) => m.id === id)
    openTab({ id: terminalTabId(id), type: 'terminal', title: meta?.title ?? `Terminal ${id}`, termId: id })
  }, [workspace, openTab])

  const focusOrOpenTerminal = useCallback(() => {
    const metas = useTerminalStore.getState().sessionsForContext(workspace)
    const last = metas[metas.length - 1]
    if (last) setActiveTabId(terminalTabId(last.id))
    else openTerminal()
  }, [workspace, openTerminal])

  const fallbackHandledTerminalOpenKeyRef = useRef(0)
  const handledTerminalOpenKeyRef = parentHandledTerminalOpenKeyRef ?? fallbackHandledTerminalOpenKeyRef
  useEffect(() => {
    if (handledTerminalOpenKeyRef.current === null) {
      handledTerminalOpenKeyRef.current = 0
    }
    if (terminalOpenKey > handledTerminalOpenKeyRef.current) {
      handledTerminalOpenKeyRef.current = terminalOpenKey
      focusOrOpenTerminal()
    }
  }, [terminalOpenKey, focusOrOpenTerminal, handledTerminalOpenKeyRef])

  const fallbackHandledViewRequestKeyRef = useRef(0)
  const handledViewRequestKeyRef = parentHandledViewRequestKeyRef ?? fallbackHandledViewRequestKeyRef
  useEffect(() => {
    if (!viewRequest || viewRequest.key <= handledViewRequestKeyRef.current) return
    handledViewRequestKeyRef.current = viewRequest.key
    openTab(viewTab(viewRequest.view))
  }, [viewRequest, openTab, handledViewRequestKeyRef])

  const fallbackHandledDiffRequestKeyRef = useRef(0)
  const handledDiffRequestKeyRef = parentHandledDiffRequestKeyRef ?? fallbackHandledDiffRequestKeyRef
  useEffect(() => {
    if (!diffRequest || diffRequest.key <= handledDiffRequestKeyRef.current) return
    handledDiffRequestKeyRef.current = diffRequest.key
    openDiffTab({
      path: diffRequest.path,
      status: diffRequest.status ?? 'M',
      additions: 0,
      deletions: 0,
    })
  }, [diffRequest, openDiffTab, handledDiffRequestKeyRef])

  const fallbackHandledPreviewRequestKeyRef = useRef(0)
  const handledPreviewRequestKeyRef = parentHandledPreviewRequestKeyRef ?? fallbackHandledPreviewRequestKeyRef
  useEffect(() => {
    if (!previewRequest || previewRequest.key <= handledPreviewRequestKeyRef.current) return
    handledPreviewRequestKeyRef.current = previewRequest.key
    openPreviewTab(previewRequest.target, { focusOnly: previewRequest.focusOnly })
  }, [previewRequest, openPreviewTab, handledPreviewRequestKeyRef])

  const activeView: DockView | null =
    activeTab?.type === 'tasks' || activeTab?.type === 'schedule' || activeTab?.type === 'plan' ? activeTab.type : null
  const onActiveViewChangeRef = useRef(onActiveViewChange)
  useEffect(() => {
    onActiveViewChangeRef.current = onActiveViewChange
  }, [onActiveViewChange])
  useEffect(() => {
    onActiveViewChangeRef.current?.(activeView)
  }, [activeView])
  useEffect(() => () => onActiveViewChangeRef.current?.(null), [])

  useEffect(() => {
    // Switching an already-mounted panel to a chat workspace drops the stale
    // Git tab (the root is not a repository).
    if (chatWorkspace && activeTabId === REVIEW_TAB_ID) {
      setActiveTabId('')
      return
    }
    if (activeTabId === REVIEW_TAB_ID || activeTabId === defaultTabId) return
    const termId = terminalIdFromTabId(activeTabId)
    const known = tabs.some((tab) => tab.id === activeTabId)
    const liveTerminal = termId !== null && terminalMetas.some((m) => m.id === termId)
    if (!known && !liveTerminal) setActiveTabId(defaultTabId)
  }, [tabs, activeTabId, terminalMetas, defaultTabId, chatWorkspace])

  const closeTab = (id: string) => {
    if (id === REVIEW_TAB_ID) return
    const target = tabs.find((item) => item.id === id)
    if (target?.type === 'terminal') {
      useTerminalStore.getState().close(target.termId)
    }
    setTabs((current) => current.filter((item) => item.id !== id))
    if (target) onTabClosed?.(target)
    if (activeTabId === id) {
      // Editor convention: focus the neighbour on the left, else the right.
      const index = visibleTabs.findIndex((item) => item.id === id)
      const neighbour = visibleTabs.filter((item) => item.id !== id)[Math.max(0, index - 1)]
      setActiveTabId(neighbour?.id ?? defaultTabId)
      onFileSelect?.(neighbour?.type === 'file' ? neighbour.file : null)
    }
  }

  // ⌘W on a terminal whose shell is still running asks first: the key is
  // easy to hit while typing in it, and closing stops the shell. The tab's ×
  // button is a deliberate click and closes right away.
  const [confirmCloseTabId, setConfirmCloseTabId] = useState<string | null>(null)
  // With no closable tab the key is left alone, so the desktop's native
  // Close Window still works; behind a dialog the dispatcher swallows it.
  useAppShortcut('closeTab', () => {
    if (activeTab?.type === 'terminal') {
      const status = useTerminalStore.getState().sessions[activeTab.termId]?.status
      if (status === 'connected' || status === 'connecting') {
        setConfirmCloseTabId(activeTab.id)
        return
      }
    }
    closeTab(activeTabId)
  }, {
    enabled: open && activeTab !== undefined && activeTab.id === activeTabId && activeTab.type !== 'review',
  })
  const confirmCloseTab = () => {
    if (confirmCloseTabId) closeTab(confirmCloseTabId)
    setConfirmCloseTabId(null)
  }
  const cancelCloseTab = () => setConfirmCloseTabId(null)

  return {
    tabs,
    visibleTabs,
    activeTabId,
    setActiveTabId,
    activeTab,
    terminalMetas,
    openTab,
    openFileTab,
    openDiffTab,
    openCommitTab,
    openPreviewTab,
    openTerminal,
    closeTab,
    confirmCloseTabId,
    confirmCloseTab,
    cancelCloseTab,
  }
}
