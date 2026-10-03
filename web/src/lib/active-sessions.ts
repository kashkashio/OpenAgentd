import type { InfiniteData } from '@tanstack/react-query'
import type { SessionPageResponse, SessionResponse } from '@/api/types'

/**
 * Sessions waiting on the user, from ``useActiveSessionsQuery`` data.
 * Filtered here rather than trusted: an older server ignores ``active`` and
 * answers with a normal page of sessions.
 */
export function needsYouSessions(data: InfiniteData<SessionPageResponse> | undefined): SessionResponse[] {
  return topLevelSessions(data).filter((session) => session.needs_input === true)
}

function topLevelSessions(data: InfiniteData<SessionPageResponse> | undefined): SessionResponse[] {
  return (data?.pages.flatMap((page) => page.data) ?? []).filter(
    (session) => !session.parent_session_id && Boolean(session.workspace),
  )
}

/**
 * Whether any session, subagents included, is making model calls. A session
 * waiting on the user reports `running` too, but spends nothing.
 */
export function sessionsSpending(data: InfiniteData<SessionPageResponse> | undefined): boolean {
  return (data?.pages ?? []).some((page) => page.data.some((s) => s.running === true && s.needs_input !== true))
}
