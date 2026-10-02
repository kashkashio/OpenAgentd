import React, { useRef } from 'react'
import { afterEach, describe, expect, it } from 'bun:test'
import { act, cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'

import { useFocusZone, type FocusZoneOptions } from '@/lib/focus/zones'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'

afterEach(cleanup)

const frame = () => act(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())))

function Zone({ children, ...options }: Partial<FocusZoneOptions> & { children: React.ReactNode }) {
  const ref = useRef<HTMLDivElement>(null)
  useFocusZone(ref, { orientation: 'vertical', ...options })
  return <div ref={ref} data-testid="zone">{children}</div>
}

function List({ count = 3, ...options }: Partial<FocusZoneOptions> & { count?: number }) {
  return (
    <>
      <button type="button">before</button>
      <Zone {...options}>
        {Array.from({ length: count }, (_, i) => <button key={i} type="button">item {i}</button>)}
      </Zone>
      <button type="button">after</button>
    </>
  )
}

const item = (i: number) => screen.getByRole('button', { name: `item ${i}` })

describe('useFocusZone', () => {
  it('is one Tab stop that enters on the first item and leaves in one press', async () => {
    const user = userEvent.setup()
    render(<List />)
    expect([0, 1, 2].map((i) => item(i).tabIndex)).toEqual([0, -1, -1])
    await user.tab()
    await user.tab()
    expect(document.activeElement).toBe(item(0))
    await user.tab()
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'after' }))
  })

  it('moves with arrows, Home and End, and remembers the item for the next Tab', async () => {
    const user = userEvent.setup()
    render(<List />)
    item(0).focus()
    await user.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(item(1))
    await user.keyboard('{End}')
    expect(document.activeElement).toBe(item(2))
    await user.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(item(2))
    await user.keyboard('{Home}')
    await user.keyboard('{ArrowDown}')
    await user.tab()
    await user.tab({ shift: true })
    expect(document.activeElement).toBe(item(1))
  })

  it('wraps when asked and uses the orientation axis', async () => {
    const user = userEvent.setup()
    render(<List orientation="horizontal" wrap />)
    item(0).focus()
    await user.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(item(0))
    await user.keyboard('{ArrowLeft}')
    expect(document.activeElement).toBe(item(2))
    await user.keyboard('{ArrowRight}')
    expect(document.activeElement).toBe(item(0))
  })

  it('enters on the last or the active item', () => {
    const { unmount } = render(<List entry="last" />)
    expect(item(2).tabIndex).toBe(0)
    unmount()
    render(
      <Zone entry="active">
        <button type="button">a</button>
        <div aria-current="page"><button type="button">b</button></div>
      </Zone>,
    )
    expect(screen.getByRole('button', { name: 'b' }).tabIndex).toBe(0)
    expect(screen.getByRole('button', { name: 'a' }).tabIndex).toBe(-1)
  })

  it('skips text fields, skipped actions, disabled and natively untabbable controls', async () => {
    const user = userEvent.setup()
    render(
      <Zone>
        <button type="button">one</button>
        <input aria-label="field" />
        <span data-zone-skip><button type="button">hover action</button></span>
        <button type="button" disabled>disabled</button>
        <div tabIndex={-1} data-testid="scroller" />
        <button type="button">two</button>
      </Zone>,
    )
    expect(screen.getByLabelText('field').tabIndex).toBe(0)
    expect(screen.getByTestId('scroller').tabIndex).toBe(-1)
    screen.getByRole('button', { name: 'one' }).focus()
    await user.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'two' }))
  })

  it('leaves nested zones and tablists their own keys', async () => {
    const user = userEvent.setup()
    render(
      <Zone>
        <button type="button">outer</button>
        <Tabs defaultValue="a">
          <TabsList aria-label="views">
            <TabsTrigger value="a">A</TabsTrigger>
            <TabsTrigger value="b">B</TabsTrigger>
          </TabsList>
        </Tabs>
        <Zone orientation="horizontal">
          <button type="button">inner 1</button>
          <button type="button">inner 2</button>
        </Zone>
      </Zone>,
    )
    const outer = screen.getByRole('button', { name: 'outer' })
    expect(screen.getByRole('button', { name: 'inner 1' }).tabIndex).toBe(0)
    outer.focus()
    await user.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(screen.getByRole('tab', { name: 'A' }))
    await user.keyboard('{ArrowRight}')
    expect(document.activeElement).toBe(screen.getByRole('tab', { name: 'B' }))
    await frame()
    expect(screen.getByRole('tab', { name: 'A' }).tabIndex).toBe(-1)
    screen.getByRole('button', { name: 'inner 1' }).focus()
    await user.keyboard('{ArrowRight}')
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'inner 2' }))
    expect(outer.tabIndex).toBe(-1)
  })

  it('falls back when the current item goes away and picks up new items', async () => {
    const { rerender } = render(<List count={3} />)
    item(2).focus()
    expect(item(2).tabIndex).toBe(0)
    rerender(<List count={2} />)
    await frame()
    expect(item(0).tabIndex).toBe(0)
    rerender(<List count={4} />)
    await frame()
    expect(item(3).tabIndex).toBe(-1)
  })

  it('sets the keyboard position on click', async () => {
    const user = userEvent.setup()
    render(<List />)
    await user.click(item(1))
    expect(document.activeElement).toBe(item(1))
    await user.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(item(2))
  })

  it('sets toolbar semantics', () => {
    render(<List orientation="horizontal" role="toolbar" label="Actions" />)
    const zone = screen.getByRole('toolbar', { name: 'Actions' })
    expect(zone.getAttribute('aria-orientation')).toBe('horizontal')
  })
})
