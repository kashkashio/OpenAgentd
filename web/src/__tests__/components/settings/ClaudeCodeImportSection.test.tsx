import { afterEach, beforeEach, describe, expect, it, mock, spyOn } from 'bun:test'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

import { ClaudeCodeImportSection } from '@/components/settings/ClaudeCodeImportSection'

const originalFetch = globalThis.fetch
let bodies: Array<Record<string, unknown>> = []

const report = (dry: boolean) => ({
  dry_run: dry,
  messages: 120,
  subagents: 2,
  workspaces: 1,
  sessions: [
    { id: 'a', title: 'Fix login', workspace: '/w', status: 'new', messages: 120, subagents: 2, detail: null },
    { id: 'b', title: 'Mine', workspace: '/w', status: 'skipped', messages: 0, subagents: 0, detail: null },
  ],
})

beforeEach(() => {
  bodies = []
  globalThis.fetch = mock(async (...args: unknown[]) => {
    const init = args[1] as RequestInit | undefined
    const body = JSON.parse(String(init?.body ?? '{}')) as Record<string, unknown>
    bodies.push(body)
    return new Response(JSON.stringify(report(Boolean(body.dry_run))), { status: 200, headers: { 'Content-Type': 'application/json' } })
  }) as typeof fetch
})

afterEach(() => {
  cleanup()
  globalThis.fetch = originalFetch
})

function renderCard() {
  const client = new QueryClient()
  const invalidate = spyOn(client, 'invalidateQueries')
  render(
    <QueryClientProvider client={client}>
      <ClaudeCodeImportSection />
    </QueryClientProvider>,
  )
  return invalidate
}

describe('ClaudeCodeImportSection', () => {
  it('previews with Check and writes nothing', async () => {
    const invalidate = renderCard()
    fireEvent.click(screen.getByRole('button', { name: 'Check' }))
    expect(await screen.findByText(/1 new, 1 already in OpenAgentd\. Would import 120 messages and 2 sub-agent sessions\./)).toBeTruthy()
    expect(bodies[0]).toEqual({ dry_run: true, workflows: false })
    expect(invalidate).not.toHaveBeenCalled()
  })

  it('imports, lists what changed, and refreshes the sidebar', async () => {
    const invalidate = renderCard()
    fireEvent.click(screen.getByLabelText('Include workflow runs'))
    fireEvent.click(screen.getByRole('button', { name: 'Import' }))
    expect(await screen.findByText(/Imported 120 messages/)).toBeTruthy()
    expect(screen.getByText(/\+ Fix login · 120 messages · 2 sub-agents/)).toBeTruthy()
    expect(bodies[0]).toEqual({ dry_run: false, workflows: true })
    await waitFor(() => expect(invalidate).toHaveBeenCalledTimes(2))
  })
})
