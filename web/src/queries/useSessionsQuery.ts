import { useInfiniteQuery, useMutation, useQueryClient, useQuery } from '@tanstack/react-query'
import { listSessions, deleteSession, updateSessionTitle, listSubagents } from '@/api/client'
import type { SessionPageResponse } from '@/api/types'
import { queryKeys } from './keys'
import { applySessionRename } from './session-rename'
import { removeSubagent } from '@/stores/cache-invalidation-bridge'

const PAGE_SIZE = 20
const CODING_WORKSPACE_PAGE_SIZE = 5
const CODING_WORKSPACE_SMOOTHING_MS = 5000

export function useSessionsQuery() {
  return useInfiniteQuery({
    queryKey: queryKeys.session.sessions.workspace('__all_coding__'),
    queryFn: ({ pageParam, signal }) =>
      listSessions(pageParam, PAGE_SIZE, undefined, signal),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage: SessionPageResponse) =>
      lastPage.has_more ? lastPage.next_cursor : undefined,
  })
}

/**
 * Sessions in one workspace, or in several checkouts at once (a repository
 * and its worktrees). A single path keeps the per-workspace key and filter;
 * several use the v3 ``workspaces`` filter, which an older server ignores, so
 * callers filter the rows they show.
 */
export function useWorkspaceSessionsQuery(workspace: string | readonly string[], enabled = true) {
  const paths = typeof workspace === 'string' ? [workspace] : workspace
  const single = paths.length === 1 ? paths[0] : null
  return useInfiniteQuery({
    queryKey: single !== null
      ? queryKeys.session.sessions.workspace(single)
      : queryKeys.session.sessions.checkouts(paths),
    queryFn: ({ pageParam, signal }) =>
      listSessions(
        pageParam,
        CODING_WORKSPACE_PAGE_SIZE,
        single !== null ? { workspace: single } : { workspaces: paths },
        signal,
      ),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage: SessionPageResponse) =>
      lastPage.has_more ? lastPage.next_cursor : undefined,
    enabled: enabled && paths.length > 0,
    staleTime: CODING_WORKSPACE_SMOOTHING_MS,
    // The global event stream patches these rows in place and resyncs after
    // any gap; a focus refetch re-read every loaded page of every workspace
    // in the sidebar, one request per page, on each alt-tab.
    refetchOnWindowFocus: false,
  })
}

/**
 * Every session running or waiting on the user, whatever page it sits on.
 * Kept in the infinite-list shape so the in-place row patches (turn state,
 * titles) reach it like any other session list; the global event stream
 * refetches it when a session joins or leaves. A v2 server ignores ``active``
 * and returns a normal page, so callers filter the rows they show.
 */
export function useActiveSessionsQuery() {
  return useInfiniteQuery({
    queryKey: queryKeys.session.sessions.active(),
    queryFn: ({ signal }) => listSessions(null, PAGE_SIZE, { active: true }, signal),
    initialPageParam: null as string | null,
    getNextPageParam: () => undefined,
  })
}

const SEARCH_PAGE_SIZE = 30

/**
 * Sessions whose title contains ``query``, newest first (first page only).
 * An older server ignores ``q`` and sends a normal page, so callers filter
 * the rows they show.
 */
export function useSessionSearchQuery(query: string) {
  return useInfiniteQuery({
    queryKey: queryKeys.session.sessions.search(query),
    queryFn: ({ signal }) => listSessions(null, SEARCH_PAGE_SIZE, { query }, signal),
    initialPageParam: null as string | null,
    getNextPageParam: () => undefined,
    enabled: query.length > 0,
  })
}

export function useUpdateSessionTitleMutation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, title }: { id: string; title: string }) => updateSessionTitle(id, title),
    onSuccess: (updated) => applySessionRename(queryClient, updated),
  })
}

export function useDeleteSessionMutation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async (target: string | { id: string; parent_session_id?: string | null }) => {
      const id = typeof target === 'string' ? target : target.id
      const parentId = typeof target === 'string' ? null : target.parent_session_id
      await deleteSession(id)
      removeSubagent(queryClient, id, parentId)
      return { id, parentId }
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.session.sessions.all() })
      queryClient.invalidateQueries({ queryKey: ['session', 'subagents'] })
    },
  })
}

export function useSessionSubagentsQuery(sessionId: string | null | undefined, enabled = true) {
  return useQuery({
    queryKey: queryKeys.session.subagents(sessionId ?? ''),
    queryFn: () => listSubagents(sessionId!),
    enabled: Boolean(sessionId) && enabled,
    staleTime: 5000,
  })
}
