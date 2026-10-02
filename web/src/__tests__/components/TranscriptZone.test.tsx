/**
 * The transcript as one focus zone, and scroll memory per session.
 *
 * However many tool calls and message actions a transcript has, it is one
 * Tab stop, entered on its newest control, with Up/Down between controls.
 * Leaving a session scrolled up and coming back restores where you were;
 * a session left at the bottom keeps following.
 */
import { afterEach, beforeEach, describe, expect, it } from 'bun:test'
import { act, cleanup, render } from '@testing-library/react'
import { AgentView } from '@/components/AgentView'
import { useAgentStore } from '@/stores/useAgentStore'
import { _resetScrollMemoryForTests } from '@/hooks/useAutoFollowScroll'
import type { ContentBlock } from '@/api/types'

beforeEach(() => {
  _resetScrollMemoryForTests()
})
afterEach(() => {
  cleanup()
  act(() => {
    useAgentStore.setState({ sessionId: null })
  })
})

function tool(id: string): ContentBlock {
  return { id, type: 'tool', content: '', toolName: 'read', toolArgs: '{"path":"a.ts"}', toolResult: 'ok', toolDone: true }
}
function text(id: string, content = 'Hello'): ContentBlock {
  return { id, type: 'text', content }
}

const sleep = (ms: number) => act(async () => { await new Promise((resolve) => setTimeout(resolve, ms)) })
const frame = () => act(async () => { await new Promise((resolve) => requestAnimationFrame(() => resolve(null))) })

const scroller = (container: HTMLElement) => container.querySelector('.oa-chat-scroll') as HTMLDivElement
const tabStops = (root: HTMLElement) =>
  Array.from(root.querySelectorAll<HTMLElement>('button, a[href], [tabindex]')).filter((el) => el.tabIndex === 0)

describe('transcript focus zone', () => {
  const fifty = [{ id: 'u1', type: 'user', content: 'Go' } as ContentBlock, ...Array.from({ length: 50 }, (_, i) => tool(`t${i}`))]

  it('is one Tab stop however many tool calls it has, entered on the newest', async () => {
    const { container } = render(<AgentView blocks={fifty} currentBlocks={[]} isWorking={false} />)
    await frame()
    const root = scroller(container)
    const headers = Array.from(root.querySelectorAll<HTMLElement>('button[aria-expanded]'))
    expect(headers.length).toBe(50)
    const stops = tabStops(root)
    expect(stops).toHaveLength(1)
    expect(stops[0]).toBe(headers.at(-1)!)
  })

  it('moves with Up and Down, and keeps its place while the stream appends', async () => {
    const { container, rerender } = render(<AgentView blocks={fifty} currentBlocks={[]} isWorking />)
    await frame()
    const root = scroller(container)
    const headers = () => Array.from(root.querySelectorAll<HTMLElement>('button[aria-expanded]'))
    act(() => headers().at(-1)!.focus())

    await act(async () => {
      document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true, cancelable: true }))
    })
    expect(document.activeElement).toBe(headers().at(-2)!)

    await act(async () => {
      rerender(<AgentView blocks={fifty} currentBlocks={[tool('live1'), tool('live2')]} isWorking />)
    })
    await sleep(250)
    const stops = tabStops(root)
    expect(stops).toHaveLength(1)
    expect(stops[0]).toBe(headers()[48])
  })
})

describe('scroll memory', () => {
  const blocksFor = (prefix: string) => Array.from({ length: 10 }, (_, i) => text(`${prefix}${i}`, `${prefix} block ${i}`))
  let restoreRect: (() => void) | null = null

  /** Blocks are 100 px tall, stacked; the scroller's viewport starts at 0. */
  function layOut(root: HTMLDivElement) {
    Object.defineProperty(root, 'scrollHeight', { value: 1000, configurable: true, writable: true })
    Object.defineProperty(root, 'clientHeight', { value: 500, configurable: true, writable: true })
    Object.defineProperty(root, 'scrollTop', { value: 500, configurable: true, writable: true })
    const original = HTMLElement.prototype.getBoundingClientRect
    HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
      const id = this.dataset.blockId
      if (id) {
        const top = Number(id.slice(1)) * 100 - root.scrollTop
        return { top, bottom: top + 100, left: 0, right: 800, width: 800, height: 100, x: 0, y: top, toJSON: () => ({}) }
      }
      if (this === root) return { top: 0, bottom: 500, left: 0, right: 800, width: 800, height: 500, x: 0, y: 0, toJSON: () => ({}) }
      return original.call(this)
    }
    restoreRect = () => { HTMLElement.prototype.getBoundingClientRect = original }
  }
  afterEach(() => {
    restoreRect?.()
    restoreRect = null
  })

  async function scrollTo(root: HTMLDivElement, top: number) {
    root.scrollTop = top
    await act(async () => { root.dispatchEvent(new Event('scroll')) })
    await frame()
  }

  async function show(rerender: (ui: React.ReactElement) => void, session: string, prefix: string) {
    await act(async () => {
      useAgentStore.setState({ sessionId: session })
      rerender(<AgentView blocks={blocksFor(prefix)} currentBlocks={[]} isWorking={false} />)
    })
    await frame()
  }

  it('puts a session left scrolled up back where it was', async () => {
    act(() => useAgentStore.setState({ sessionId: 'a' }))
    const { container, rerender } = render(<AgentView blocks={blocksFor('a')} currentBlocks={[]} isWorking={false} />)
    const root = scroller(container)
    layOut(root)
    await scrollTo(root, 500)
    // Up to 250: block a2 is the first in view, 50 px above the top.
    await scrollTo(root, 250)
    expect(container.querySelector('button[aria-label="Scroll to bottom"]')).toBeTruthy()

    await show(rerender, 'b', 'b')
    expect(root.scrollTop).toBe(500)

    await show(rerender, 'a', 'a')
    expect(root.scrollTop).toBe(250)
    expect(container.querySelector('button[aria-label="Scroll to bottom"]')).toBeTruthy()
  })

  it('keeps following a session left at the bottom', async () => {
    act(() => useAgentStore.setState({ sessionId: 'a' }))
    const { container, rerender } = render(<AgentView blocks={blocksFor('a')} currentBlocks={[]} isWorking={false} />)
    const root = scroller(container)
    layOut(root)
    await scrollTo(root, 500)
    await scrollTo(root, 250)
    // Back down to the end before leaving.
    await scrollTo(root, 500)

    await show(rerender, 'b', 'b')
    root.scrollTop = 0
    await show(rerender, 'a', 'a')
    expect(root.scrollTop).toBe(500)
    expect(container.querySelector('button[aria-label="Scroll to bottom"]')).toBeNull()
  })
})
