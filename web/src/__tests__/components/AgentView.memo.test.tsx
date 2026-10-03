/**
 * Performance regression: streaming must re-render only the live turn.
 *
 * ``AssistantTurn`` is wrapped in ``memo``, but AgentView handed every turn a
 * fresh inline ``renderBlock`` closure (it captured a ``Set`` rebuilt on every
 * token) and the whole-transcript ``totalBlocks``/``finalizedCount``, so each
 * streamed flush re-rendered every visible turn. The React Compiler cannot
 * cache a closure created inside the turns' ``.map`` callback, so this holds
 * in production builds too.
 *
 * ``AssistantTurn`` is swapped for a recorder behind the same plain ``memo``,
 * so a render here means the real component's memo would have missed.
 */
import { afterEach, beforeEach, describe, expect, it, mock } from 'bun:test'
import { cleanup, render } from '@testing-library/react'
import { memo } from 'react'

mock.module('lucide-react', () => new Proxy({}, { get: () => () => null }))

const turnRenders = new Map<string, number>()
mock.module('@/components/AssistantTurnFooter', () => ({
  AssistantTurnFooter: () => null,
  AssistantTurn: memo(function RecordingTurn({ blocks }: { blocks: ContentBlock[] }) {
    const id = blocks[0]?.id ?? ''
    turnRenders.set(id, (turnRenders.get(id) ?? 0) + 1)
    return null
  }),
}))

import { AgentView } from '@/components/AgentView'
import { useAgentStore } from '@/stores/useAgentStore'
import type { ContentBlock } from '@/api/types'

beforeEach(() => {
  useAgentStore.setState({ sessionId: 'session-1' })
  turnRenders.clear()
})

afterEach(() => {
  cleanup()
  useAgentStore.setState({ sessionId: null })
})

const FINALIZED: ContentBlock[] = [
  { id: 'u1', type: 'user', content: 'first prompt' },
  { id: 'a1', type: 'text', content: 'first answer' },
  {
    id: 'app1',
    type: 'tool',
    content: '',
    toolName: 'show_chart',
    toolCallId: 'app1',
    toolDone: true,
    toolResult: 'ok',
    extra: { mcp_app: { resource_uri: 'ui://chart' } },
  },
  { id: 'u2', type: 'user', content: 'second prompt' },
  { id: 'a2', type: 'text', content: 'second answer' },
]

const PROMPT: ContentBlock = { id: 'u3', type: 'user', content: 'third prompt' }

describe('AgentView — streaming re-renders only the live turn', () => {
  it('keeps finished turns still while text streams and new blocks open', () => {
    const { rerender } = render(
      <AgentView blocks={FINALIZED} currentBlocks={[PROMPT, { id: 'live', type: 'text', content: 'Th' }]} isWorking />,
    )
    expect(turnRenders.get('a1')).toBe(1)
    turnRenders.clear()

    rerender(<AgentView blocks={FINALIZED} currentBlocks={[PROMPT, { id: 'live', type: 'text', content: 'Thinking' }]} isWorking />)
    rerender(
      <AgentView
        blocks={FINALIZED}
        currentBlocks={[
          PROMPT,
          { id: 'live', type: 'text', content: 'Thinking about it' },
          { id: 'tool-1', type: 'tool', content: '', toolName: 'read', toolCallId: 'tool-1' },
        ]}
        isWorking
      />,
    )

    expect(turnRenders.get('live')).toBeGreaterThan(0)
    expect(turnRenders.get('a1')).toBeUndefined()
    expect(turnRenders.get('a2')).toBeUndefined()
  })
})
