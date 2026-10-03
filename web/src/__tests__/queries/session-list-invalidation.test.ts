/**
 * The cockpit refreshes session lists with one
 * ``invalidateQueries(sessions.all())`` when a new session starts. That
 * relies on the infinite list living under the ``sessions.all()`` prefix: a
 * follow-up ``refetchQueries(sessions.infinite())`` would only cancel the
 * in-flight refetch and send the request again.
 */
import { describe, it, expect } from 'bun:test'
import { InfiniteQueryObserver, QueryClient } from '@tanstack/react-query'
import { queryKeys } from '@/queries/keys'

async function settle(client: QueryClient) {
  for (let i = 0; i < 20 && client.isFetching() > 0; i++) await new Promise((r) => setTimeout(r, 5))
}

describe('session list invalidation', () => {
  it('refetches an active infinite session list once per invalidate', async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    let calls = 0
    const observer = new InfiniteQueryObserver(client, {
      queryKey: queryKeys.session.sessions.infinite(),
      queryFn: async () => {
        calls++
        await new Promise((r) => setTimeout(r, 10))
        return { items: [] as string[] }
      },
      initialPageParam: 0,
      getNextPageParam: () => undefined,
    })
    const unsubscribe = observer.subscribe(() => {})
    await settle(client)
    calls = 0

    await client.invalidateQueries({ queryKey: queryKeys.session.sessions.all() })
    await settle(client)
    expect(calls).toBe(1)
    unsubscribe()
    client.clear()
  })
})
