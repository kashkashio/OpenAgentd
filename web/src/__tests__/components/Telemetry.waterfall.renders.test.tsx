/**
 * Selecting a span in a large trace re-renders only the rows whose
 * selection changes. Each row has one tooltip, so tooltip renders count rows.
 */
import { afterEach, describe, expect, it, mock } from 'bun:test'
import { act, cleanup, render } from '@testing-library/react'
import type { ReactElement, ReactNode } from 'react'
import type { SpanDetail } from '@/api/client'

afterEach(cleanup)

mock.module('lucide-react', () => new Proxy({}, { get: () => () => null }))
let rowRenders = 0
mock.module('@/components/ui/tooltip', () => ({
  Tooltip: ({ children }: { children: ReactNode }) => {
    rowRenders += 1
    return <>{children}</>
  },
  TooltipTrigger: ({ render }: { render: ReactElement }) => render,
  TooltipContent: () => null,
}))

import { Waterfall } from '@/components/Telemetry/Waterfall'

const spans: SpanDetail[] = Array.from({ length: 300 }, (_, i) => ({
  span_id: `s${i}`,
  parent_span_id: i ? `s${Math.floor((i - 1) / 4)}` : null,
  trace_id: 't',
  name: i % 2 ? 'chat gpt-5' : 'execute_tool read',
  kind: 'INTERNAL',
  start_ms: i,
  end_ms: i + 5,
  duration_ms: 5,
  status: 'OK',
  attributes: {},
}))

describe('Waterfall — selection', () => {
  it('re-renders only the previously and newly selected rows', () => {
    const onSelect = () => {}
    const view = render(<Waterfall spans={spans} selectedSpanId="s10" onSelectSpan={onSelect} />)
    expect(rowRenders).toBe(300)
    rowRenders = 0
    act(() => view.rerender(<Waterfall spans={spans} selectedSpanId="s20" onSelectSpan={onSelect} />))
    expect(rowRenders).toBe(2)
  })
})
