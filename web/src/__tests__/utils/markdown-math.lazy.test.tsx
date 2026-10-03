/**
 * KaTeX (and its stylesheet) load on first use instead of with the app
 * shell. This file runs in its own worker (`bun test --parallel`), so KaTeX
 * has not been loaded yet when the first formula renders.
 */
import { describe, it, expect, afterEach } from 'bun:test'
import { render, cleanup, waitFor } from '@testing-library/react'
import { MarkdownBlock } from '@/utils/markdown'

afterEach(cleanup)

describe('lazy KaTeX', () => {
  it('shows the formula source until KaTeX loads, then renders it', async () => {
    const { container } = render(<MarkdownBlock content={'Arrow: $\\rightarrow$ and $$E = mc^2$$'} />)
    const inline = container.querySelector('.oa-math-inline')
    expect(inline?.querySelector('.katex')).toBeNull()
    expect(inline?.textContent).toBe('\\rightarrow')

    await waitFor(() => expect(container.querySelector('.oa-math-inline .katex')).not.toBeNull())
    expect(container.querySelector('.oa-math-inline')?.textContent).toContain('→')
  })

  it('renders math synchronously once KaTeX is loaded', async () => {
    const first = render(<MarkdownBlock content="$x$" />)
    await waitFor(() => expect(first.container.querySelector('.katex')).not.toBeNull())
    cleanup()

    const { container } = render(<MarkdownBlock content={'$$\\int_0^1 x\\,dx$$'} />)
    expect(container.querySelector('.oa-math-block .katex-display')).not.toBeNull()
  })
})
