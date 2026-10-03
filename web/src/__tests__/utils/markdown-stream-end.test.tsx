/**
 * Ending a stream must not remount the message's rendered markdown.
 *
 * The renderer creates each ``components`` entry as a React component type.
 * The map was rebuilt when ``isStreaming`` flipped, so the moment a message
 * finished, every code block, table cell, image and plan card in it was
 * unmounted and mounted again: copy/collapse state reset, images flashed
 * and re-decoded, and the transcript jumped while heights settled.
 */
import { afterEach, describe, expect, it, mock } from 'bun:test'
import { cleanup, render } from '@testing-library/react'

mock.module('lucide-react', () => new Proxy({}, { get: () => () => null }))

import { MarkdownBlock } from '@/utils/markdown'

afterEach(cleanup)

const MESSAGE = [
  'Here is the fix:',
  '',
  '```ts',
  'const answer = 42',
  '```',
  '',
  '| file | change |',
  '| --- | --- |',
  '| a.ts | edited |',
  '',
  '![chart](chart.png)',
].join('\n')

describe('MarkdownBlock — the end of a stream', () => {
  it('keeps the same code, table and image elements when streaming stops', () => {
    const { container, rerender } = render(<MarkdownBlock content={MESSAGE} sessionId="s1" isStreaming />)
    const pre = container.querySelector('pre')
    const cell = container.querySelector('td')
    const image = container.querySelector('img')
    expect(pre).not.toBeNull()
    expect(cell).not.toBeNull()
    expect(image).not.toBeNull()

    rerender(<MarkdownBlock content={MESSAGE} sessionId="s1" isStreaming={false} />)

    expect(container.querySelector('pre')).toBe(pre)
    expect(container.querySelector('td')).toBe(cell)
    expect(container.querySelector('img')).toBe(image)
  })

  it('still highlights a code block once its stream ends', () => {
    const { container, rerender } = render(<MarkdownBlock content={MESSAGE} isStreaming />)
    expect(container.querySelector('pre .th-token')).toBeNull()

    rerender(<MarkdownBlock content={MESSAGE} isStreaming={false} />)

    expect(container.querySelector('pre .th-token')).not.toBeNull()
  })
})
