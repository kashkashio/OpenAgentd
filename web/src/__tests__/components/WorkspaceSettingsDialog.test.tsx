import { afterEach, beforeEach, describe, expect, it, mock } from 'bun:test'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

import { WorkspaceSettingsDialog } from '@/components/WorkspaceSettingsDialog'

const originalFetch = globalThis.fetch
const WS = '/tmp/project'

let settings = {
  workspace: WS,
  path: `${WS}/.openagentd/settings.yaml`,
  model: null as string | null,
  thinking_level: null as string | null,
  claude_code: { permission_mode: null as string | null },
}
let puts: Array<Record<string, unknown>> = []

const registry = {
  tools: [],
  skills: [],
  providers: ['claude-code', 'openai'],
  models: [
    { id: 'claude-code:sonnet', provider: 'claude-code', model: 'sonnet', vision: false, output_image: false, output_video: false, thinking_levels: [], summary_trigger_tokens: 0, fast_mode: false },
    { id: 'openai:gpt-5', provider: 'openai', model: 'gpt-5', vision: false, output_image: false, output_video: false, thinking_levels: [], summary_trigger_tokens: 0, fast_mode: false },
  ],
}

function jsonResponse(body: unknown) {
  return new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } })
}

beforeEach(() => {
  puts = []
  settings = { ...settings, model: null, thinking_level: null, claude_code: { permission_mode: null } }
  globalThis.fetch = mock(async (...args: unknown[]) => {
    const input = args[0] as RequestInfo | URL
    const init = args[1] as RequestInit | undefined
    const url = String(input)
    if (url.includes('/agents/registry')) return jsonResponse(registry)
    if (url.includes('/agent/workspace/settings')) {
      if (init?.method === 'PUT') {
        const body = JSON.parse(String(init.body)) as Record<string, unknown>
        puts.push(body)
        settings = { ...settings, model: body.model as string | null, thinking_level: body.thinking_level as string | null, claude_code: body.claude_code as { permission_mode: string | null } }
      }
      return jsonResponse(settings)
    }
    return new Response(null, { status: 404 })
  }) as typeof fetch
})

afterEach(() => {
  cleanup()
  globalThis.fetch = originalFetch
})

function renderDialog() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  return render(
    <QueryClientProvider client={queryClient}>
      <WorkspaceSettingsDialog workspace={WS} open onOpenChange={() => {}} />
    </QueryClientProvider>,
  )
}

describe('WorkspaceSettingsDialog', () => {
  it('leaves existing sessions alone when unticked', async () => {
    renderDialog()
    fireEvent.click(await screen.findByLabelText('Also switch existing sessions'))
    fireEvent.click(await screen.findByRole('combobox', { name: 'Search session model' }))
    fireEvent.click(await screen.findByText('openai:gpt-5'))
    await waitFor(() => expect(puts.length).toBe(1))
    expect(puts[0]).toMatchObject({ model: 'openai:gpt-5', apply_to_sessions: false })
  })

  it('saves a picked model as the workspace default', async () => {
    renderDialog()
    const input = await screen.findByRole('combobox', { name: 'Search session model' })
    fireEvent.click(input)
    fireEvent.click(await screen.findByText('openai:gpt-5'))

    await waitFor(() => expect(puts.length).toBe(1))
    expect(puts[0]).toMatchObject({ workspace: WS, model: 'openai:gpt-5', apply_to_sessions: true })
    expect(await screen.findByRole('button', { name: 'Use agent default' })).toBeTruthy()
    // Not a Claude Code model: no permission control.
    expect(screen.queryByLabelText('Claude Code permission mode')).toBeNull()
  })

  it('shows Claude Code permissions for a claude-code default and clears it', async () => {
    settings = { ...settings, model: 'claude-code:sonnet' }
    renderDialog()

    expect(await screen.findByLabelText('Claude Code permission mode')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Use agent default' }))

    await waitFor(() => expect(puts.length).toBe(1))
    expect(puts[0]).toMatchObject({ workspace: WS, model: null, thinking_level: null })
  })
})
