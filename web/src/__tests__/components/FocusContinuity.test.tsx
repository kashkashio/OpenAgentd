import React, { useState } from 'react'
import { afterEach, describe, expect, it, mock } from 'bun:test'
import { act, cleanup, render, screen } from '@testing-library/react'

mock.module('@/hooks/use-mobile', () => ({ useIsMobile: () => false }))
mock.module('@/hooks/use-platform', () => ({
  usePlatform: () => ({ isTauri: false, os: 'macos', isMacOverlay: false }),
  getPlatform: () => ({ isTauri: false, os: 'macos', isMacOverlay: false }),
}))

import { InputComposer } from '@/components/InputComposer'
import { useStrandedFocusGuard } from '@/hooks/use-dock-focus'
import { _resetKeyboardForTests } from '@/lib/keyboard/dispatcher'
import { useKeyLayer } from '@/lib/keyboard/hooks'

afterEach(() => {
  cleanup()
  _resetKeyboardForTests()
})

const tab = () => {
  const event = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true })
  ;(document.activeElement ?? document.body).dispatchEvent(event)
  return event
}

function Guarded({ dialog = false }: { dialog?: boolean }) {
  const [shown, setShown] = useState(true)
  useStrandedFocusGuard(true, () => document.getElementById('home')?.focus())
  useKeyLayer(dialog, { kind: 'dialog' })
  return (
    <>
      <button type="button" id="home">composer</button>
      {shown && <button type="button" onClick={() => setShown(false)}>doomed</button>}
    </>
  )
}

describe('stranded focus guard', () => {
  it('sends a Tab that starts from <body> to the composer instead of the top of the page', () => {
    render(<Guarded />)
    const doomed = screen.getByRole('button', { name: 'doomed' })
    doomed.focus()
    act(() => doomed.click())
    expect(document.activeElement).toBe(document.body)
    const event = tab()
    expect(event.defaultPrevented).toBe(true)
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'composer' }))
  })

  it('leaves focus alone when it is on a control, or a dialog owns it', () => {
    const { unmount } = render(<Guarded />)
    screen.getByRole('button', { name: 'doomed' }).focus()
    expect(tab().defaultPrevented).toBe(false)
    unmount()
    render(<Guarded dialog />)
    ;(document.activeElement as HTMLElement | null)?.blur()
    expect(tab().defaultPrevented).toBe(false)
    expect(document.activeElement).toBe(document.body)
  })
})

describe('composer minimize keeps focus', () => {
  it('moves focus from the textarea to the pill when the bar minimizes (send, Esc)', () => {
    const { rerender } = render(<InputComposer onSubmit={() => {}} minimized={false} />)
    screen.getByRole('textbox').focus()
    rerender(<InputComposer onSubmit={() => {}} minimized />)
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Expand input bar' }))
  })

  it('does not pull focus that already moved elsewhere', () => {
    const { rerender } = render(
      <>
        <button type="button">elsewhere</button>
        <InputComposer onSubmit={() => {}} minimized={false} />
      </>,
    )
    screen.getByRole('button', { name: 'elsewhere' }).focus()
    rerender(
      <>
        <button type="button">elsewhere</button>
        <InputComposer onSubmit={() => {}} minimized />
      </>,
    )
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'elsewhere' }))
  })
})
