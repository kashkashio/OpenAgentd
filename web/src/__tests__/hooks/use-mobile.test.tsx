import { afterEach, beforeEach, describe, expect, it } from 'bun:test'
import { act, cleanup, render, screen } from '@testing-library/react'
import { useIsMobile } from '@/hooks/use-mobile'

afterEach(cleanup)

function Probe({ i }: { i: number }) {
  return <span data-testid={`p${i}`}>{useIsMobile() ? 'mobile' : 'desktop'}</span>
}

describe('useIsMobile', () => {
  let original: typeof window.matchMedia
  let calls = 0
  let matches = false
  let listeners: Array<() => void> = []

  beforeEach(() => {
    original = window.matchMedia
    calls = 0
    matches = false
    listeners = []
    // A fresh function per test, as a test that swaps matchMedia would do.
    window.matchMedia = ((query: string) => {
      calls++
      return {
        get matches() {
          return matches
        },
        media: query,
        onchange: null,
        addListener: () => {},
        removeListener: () => {},
        addEventListener: (_: string, l: () => void) => listeners.push(l),
        removeEventListener: (_: string, l: () => void) => {
          listeners = listeners.filter((x) => x !== l)
        },
        dispatchEvent: () => false,
      }
    }) as unknown as typeof window.matchMedia
  })

  afterEach(() => {
    window.matchMedia = original
  })

  it('shares one media query list across every component', () => {
    render(
      <>
        {Array.from({ length: 50 }, (_, i) => (
          <Probe key={i} i={i} />
        ))}
      </>,
    )
    expect(screen.getByTestId('p0').textContent).toBe('desktop')
    expect(calls).toBe(1)
  })

  it('updates every subscriber when the query starts matching', () => {
    render(
      <>
        <Probe i={0} />
        <Probe i={1} />
      </>,
    )
    act(() => {
      matches = true
      for (const l of listeners) l()
    })
    expect(screen.getByTestId('p0').textContent).toBe('mobile')
    expect(screen.getByTestId('p1').textContent).toBe('mobile')
  })

  it('drops its listeners on unmount', () => {
    const { unmount } = render(<Probe i={0} />)
    unmount()
    expect(listeners).toHaveLength(0)
  })
})
