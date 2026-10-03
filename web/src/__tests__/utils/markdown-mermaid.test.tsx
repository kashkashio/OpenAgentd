import React from 'react'
import { afterEach, describe, expect, it, mock } from 'bun:test'
import { cleanup, render, screen } from '@testing-library/react'

mock.module('lucide-react', () => new Proxy({}, { get: () => () => null }))

import { MarkdownBlock } from '@/utils/markdown'

const source = ['flowchart LR', '  A --> B'].join('\n')
const diagram = ['```mermaid', source, '```'].join('\n')

afterEach(cleanup)

// Chat no longer renders diagrams: a Mermaid fence is ordinary code, with the
// language label and copy action every fenced block gets.
describe('MarkdownBlock Mermaid fences', () => {
  it('renders a completed Mermaid fence as a code block', () => {
    render(<MarkdownBlock content={diagram} />)

    expect(screen.queryByRole('tab', { name: 'Diagram' })).toBeNull()
    expect(screen.queryByRole('img')).toBeNull()
    expect(screen.getByText('mermaid')).toBeTruthy()
    expect(document.querySelector('pre')?.textContent).toContain('flowchart LR')
  })

  it('renders a Mermaid fence as code while the response is streaming', () => {
    render(<MarkdownBlock content={`${diagram}\n\nStill explaining.`} isStreaming />)

    expect(screen.queryByRole('tab', { name: 'Diagram' })).toBeNull()
    expect(document.querySelector('pre')?.textContent).toContain('flowchart LR')
    expect(screen.getByText('Still explaining.')).toBeTruthy()
  })
})
