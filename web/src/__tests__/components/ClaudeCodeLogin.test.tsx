import { afterEach, beforeEach, describe, expect, it, mock } from 'bun:test'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'

const opened: Array<{ target: unknown; context: string }> = []
const typed: Array<{ id: string; data: string }> = []
const renamed: Array<{ id: string; title: string }> = []
mock.module('@/stores/useTerminalStore', () => ({
  useTerminalStore: {
    getState: () => ({
      open: (target: unknown, context: string) => {
        opened.push({ target, context })
        return 'term-1'
      },
      runWhenConnected: (id: string, data: string) => typed.push({ id, data }),
      rename: (id: string, title: string) => renamed.push({ id, title }),
    }),
  },
}))

import { ErrorCard } from '@/components/AgentView/ErrorCard'
import { isClaudeCodeLoginError } from '@/lib/claude-code-login'
import { useAgentStore } from '@/stores/useAgentStore'

const EXPIRED = 'Failed to authenticate: OAuth session expired and could not be refreshed'
let events: string[] = []
const record = (e: Event) => events.push(e.type)

beforeEach(() => {
  opened.length = 0
  typed.length = 0
  renamed.length = 0
  events = []
  window.addEventListener('oa:open-terminal', record)
})

afterEach(() => {
  cleanup()
  window.removeEventListener('oa:open-terminal', record)
  useAgentStore.setState({ _workspace: null, sessionModel: null } as never)
})

describe('Claude Code sign-in', () => {
  it('recognises login failures only in Claude Code sessions', () => {
    expect(isClaudeCodeLoginError(EXPIRED, 'claude-code:claude-opus-5')).toBe(true)
    expect(isClaudeCodeLoginError('Not logged in · Please run /login', 'claude-code:opus')).toBe(true)
    expect(isClaudeCodeLoginError(EXPIRED, 'anthropic:claude-opus-5-5')).toBe(false)
    expect(isClaudeCodeLoginError('Rate limited', 'claude-code:opus')).toBe(false)
  })

  it('offers sign-in on the error card and runs claude auth login in a new terminal', () => {
    useAgentStore.setState({ _workspace: '/w', sessionModel: 'claude-code:claude-opus-5' } as never)
    render(<ErrorCard message={EXPIRED} onRetry={() => {}} />)
    fireEvent.click(screen.getByRole('button', { name: 'Sign in to Claude Code' }))
    expect(opened).toEqual([{ target: { workspace: '/w' }, context: '/w' }])
    expect(renamed).toEqual([{ id: 'term-1', title: 'Claude login' }])
    expect(typed).toEqual([{ id: 'term-1', data: 'claude auth login\r' }])
    expect(events).toEqual(['oa:open-terminal'])
  })

  it('does not offer sign-in for other errors or models', () => {
    useAgentStore.setState({ _workspace: '/w', sessionModel: 'anthropic:claude-opus-5-5' } as never)
    render(<ErrorCard message={EXPIRED} onRetry={() => {}} />)
    expect(screen.queryByRole('button', { name: 'Sign in to Claude Code' })).toBeNull()
  })
})
