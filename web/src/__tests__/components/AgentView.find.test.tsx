import { describe, it, expect, afterEach, beforeEach, mock } from 'bun:test'
import { render, cleanup } from '@testing-library/react'
import '@testing-library/jest-dom'

mock.module('lucide-react', () => new Proxy({}, { get: () => () => null }))

import { AgentView } from '@/components/AgentView'
import {
  TRANSCRIPT_FIND_ACTIVE_HIGHLIGHT,
  TRANSCRIPT_FIND_HIGHLIGHT,
} from '@/components/AgentView/transcript-find-highlight'
import { useAgentStore } from '@/stores/useAgentStore'
import type { ContentBlock } from '@/api/types'
import { installFakeHighlights, type FakeHighlights } from './fake-highlights'

let highlights: FakeHighlights
beforeEach(() => { highlights = installFakeHighlights() })

afterEach(() => {
  cleanup()
  highlights.restore()
  useAgentStore.setState({ sessionId: null, _pendingMessages: [] })
})

const BLOCKS: ContentBlock[] = [
  { id: 'u1', type: 'user', content: 'Hello world' },
  { id: 'th1', type: 'thinking', content: 'Consider the world model' },
  { id: 'tool1', type: 'tool', content: '', toolName: 'read', toolArgs: 'world.txt', toolResult: 'world.bin' },
  { id: 'a1', type: 'text', content: 'A smaller world' },
]

function painted(): Range[] {
  return [TRANSCRIPT_FIND_ACTIVE_HIGHLIGHT, TRANSCRIPT_FIND_HIGHLIGHT].flatMap(
    (name) => highlights.registry.get(name)?.ranges ?? [],
  )
}

describe('AgentView — transcript find', () => {
  it('paints exact matches in user, thinking, and assistant text without a block ring', () => {
    const { container } = render(
      <AgentView blocks={BLOCKS} currentBlocks={[]} isWorking={false} findOpen findQuery="world" findActiveIndex={0} />,
    )

    expect(painted().map((range) => range.toString())).toEqual(['world', 'world', 'world'])
    expect(highlights.texts(TRANSCRIPT_FIND_ACTIVE_HIGHLIGHT)).toEqual(['world'])
    expect(painted().some((range) => range.startContainer.parentElement?.closest('[data-find-block="tool1"]'))).toBe(false)
    expect(container.querySelector('mark')).toBeNull()
    expect(container.querySelector('[class*="ring-1"]')).toBeNull()
  })

  // Wrapping matches in <mark>s replaced text nodes React owns, so text that
  // held a match was written to detached nodes and froze while find was open.
  // (Not streaming here: the smoothed stream eases text in on animation frames.)
  it('keeps text under a match updating while find is open', () => {
    const live = (content: string): ContentBlock[] => [{ id: 'live', type: 'text', content }]
    const { container, rerender } = render(
      <AgentView blocks={BLOCKS} currentBlocks={live('world one')} isWorking={false} findOpen findQuery="world" findActiveIndex={0} />,
    )
    rerender(
      <AgentView blocks={BLOCKS} currentBlocks={live('world one two')} isWorking={false} findOpen findQuery="world" findActiveIndex={0} />,
    )
    expect(container.querySelector('[data-block-id="live"]')?.textContent).toContain('world one two')
  })

  it('clears the highlights when find closes', () => {
    const { rerender } = render(
      <AgentView blocks={BLOCKS} currentBlocks={[]} isWorking={false} findOpen findQuery="world" findActiveIndex={0} />,
    )
    rerender(<AgentView blocks={BLOCKS} currentBlocks={[]} isWorking={false} findOpen={false} findQuery="" findActiveIndex={0} />)
    expect(painted()).toHaveLength(0)
  })
})
