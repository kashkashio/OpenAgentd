import { afterEach, beforeEach, describe, expect, it, mock } from 'bun:test'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

import { PanelResizeHandle, ResizableAside } from '@/components/ResizableAside'

// rAF is async in browsers: queue frames and flush them explicitly.
const frames: FrameRequestCallback[] = []
const realRaf = globalThis.requestAnimationFrame

beforeEach(() => {
  frames.length = 0
  globalThis.requestAnimationFrame = ((callback: FrameRequestCallback) => frames.push(callback)) as typeof requestAnimationFrame
})

afterEach(() => {
  cleanup()
  globalThis.requestAnimationFrame = realRaf
})

function moveTo(clientX: number) {
  act(() => {
    window.dispatchEvent(new MouseEvent('pointermove', { clientX }))
    while (frames.length > 0) frames.shift()?.(0)
  })
}

describe('ResizableAside', () => {
  it('drags without re-rendering the panel content, then commits once', () => {
    let contentRenders = 0
    function Content() {
      contentRenders += 1
      return <p>content</p>
    }
    const onCommit = mock(() => {})
    const motionCalls: Array<{ width: number; isResizing: boolean }> = []
    const getMotion = (live: { width: number; isResizing: boolean }) => {
      motionCalls.push(live)
      return { animate: { width: live.width }, transition: { duration: 0 } }
    }

    render(
      <ResizableAside
        aria-label="Panel"
        resize={{ width: 300, min: 200, max: 500, edge: 'right', onCommit, label: 'Resize panel' }}
        getMotion={getMotion}
      >
        <PanelResizeHandle edge="right" />
        <Content />
      </ResizableAside>,
    )
    const separator = screen.getByRole('separator', { name: 'Resize panel' })
    expect(separator.getAttribute('aria-valuenow')).toBe('300')
    const settled = contentRenders

    fireEvent.pointerDown(separator, { button: 0, clientX: 100, pointerType: 'mouse' })
    moveTo(150)
    moveTo(180)

    expect(separator.getAttribute('aria-valuenow')).toBe('380')
    expect(motionCalls.at(-1)).toEqual({ width: 380, isResizing: true })
    expect(contentRenders).toBe(settled)
    expect(onCommit).not.toHaveBeenCalled()

    act(() => { window.dispatchEvent(new MouseEvent('pointerup')) })
    expect(onCommit).toHaveBeenCalledTimes(1)
    expect(onCommit).toHaveBeenCalledWith(380)
    expect(contentRenders).toBe(settled)
  })

  it('renders no handle outside a ResizableAside', () => {
    render(<PanelResizeHandle edge="left" />)
    expect(screen.queryByRole('separator')).toBeNull()
  })

  // Closing tweens the aside's width to 0. Content that tracked that width
  // reflowed every frame (a long plan: ~10 ms a frame); pinned to the target
  // width it is only clipped by the shrinking aside.
  it('pins the content to the target width so open and close tweens only clip it', () => {
    render(
      <ResizableAside
        aria-label="Panel"
        pinContentWidth
        resize={{ width: 300, min: 200, max: 500, edge: 'right', onCommit: () => {}, label: 'Resize panel' }}
        getMotion={(live) => ({ animate: { width: live.width }, transition: { duration: 0 } })}
      >
        <PanelResizeHandle edge="right" />
        <p>content</p>
      </ResizableAside>,
    )
    const content = screen.getByText('content').parentElement as HTMLElement
    expect(content.style.width).toBe('300px')

    // A drag is a real resize: the content follows it.
    fireEvent.pointerDown(screen.getByRole('separator'), { button: 0, clientX: 100, pointerType: 'mouse' })
    moveTo(150)
    expect(content.style.width).toBe('350px')
  })

  it('lets the content fill the aside when the motion has no width target', () => {
    render(
      <ResizableAside
        aria-label="Panel"
        pinContentWidth
        resize={{ width: 300, min: 200, max: 500, edge: 'right', onCommit: () => {}, label: 'Resize panel' }}
        getMotion={() => ({ animate: { opacity: 1 }, transition: { duration: 0 } })}
      >
        <p>content</p>
      </ResizableAside>,
    )
    expect((screen.getByText('content').parentElement as HTMLElement).style.width).toBe('100%')
  })
})
