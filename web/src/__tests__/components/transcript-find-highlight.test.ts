import { afterEach, beforeEach, describe, expect, it } from 'bun:test'
import {
  TRANSCRIPT_FIND_ACTIVE_HIGHLIGHT,
  TRANSCRIPT_FIND_HIGHLIGHT,
  clearTranscriptFind,
  paintTranscriptFind,
} from '@/components/AgentView/transcript-find-highlight'
import { installFakeHighlights, type FakeHighlights } from './fake-highlights'

function mount(html: string): HTMLElement {
  const root = document.createElement('div')
  root.innerHTML = html
  document.body.appendChild(root)
  return root
}

let highlights: FakeHighlights
beforeEach(() => { highlights = installFakeHighlights() })
afterEach(() => { highlights.restore() })

describe('paintTranscriptFind', () => {
  it('paints only the matched substring and leaves the DOM untouched', () => {
    const root = mount('<div data-find-block="u1">Hello world</div>')
    const text = root.firstChild!.firstChild
    const active = paintTranscriptFind(root, 'HELLO', 0)
    expect(active?.toString()).toBe('Hello')
    expect(highlights.texts(TRANSCRIPT_FIND_ACTIVE_HIGHLIGHT)).toEqual(['Hello'])
    // React owns these nodes: nothing is wrapped, split or replaced.
    expect(root.innerHTML).toBe('<div data-find-block="u1">Hello world</div>')
    expect(root.firstChild!.firstChild).toBe(text)
    root.remove()
  })

  it('paints the active occurrence apart from the others', () => {
    const root = mount(
      '<div data-find-block="a1">foo bar foo</div><div data-find-block="a2">foo</div>',
    )
    const active = paintTranscriptFind(root, 'foo', 1)
    expect(active?.startContainer.parentElement?.getAttribute('data-find-block')).toBe('a1')
    expect(active?.startOffset).toBe(8)
    expect(highlights.texts(TRANSCRIPT_FIND_ACTIVE_HIGHLIGHT)).toEqual(['foo'])
    expect(highlights.texts(TRANSCRIPT_FIND_HIGHLIGHT)).toEqual(['foo', 'foo'])
    root.remove()
  })

  it('does not search unmarked nodes', () => {
    const root = mount('<div data-tool="t1">secret-token</div>')
    expect(paintTranscriptFind(root, 'secret', 0)).toBeNull()
    expect(highlights.registry.size).toBe(0)
    root.remove()
  })

  it('clears both highlights', () => {
    const root = mount('<div data-find-block="u1">foo foo</div>')
    paintTranscriptFind(root, 'foo', 0)
    clearTranscriptFind()
    expect(highlights.registry.size).toBe(0)
    root.remove()
  })

  it('still finds the active match where the browser cannot paint highlights', () => {
    highlights.restore()
    const root = mount('<div data-find-block="u1">Hello world</div>')
    expect(paintTranscriptFind(root, 'world', 0)?.toString()).toBe('world')
    clearTranscriptFind()
    root.remove()
  })
})
