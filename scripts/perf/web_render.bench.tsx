/**
 * Transcript render benchmark (plan Phase 2), runnable unchanged at
 * fad35386 and HEAD under happy-dom. Absolute times are not browser
 * times; the before/after ratio and the commit counts are the signal.
 *
 * 80 finished turns (markdown with a code block and three finished tools),
 * then a live turn that streams 300 flushes of 16 characters and opens a new
 * tool every 30 flushes. Every flush is one rerender, as the store would cause.
 */
import { afterEach, it, mock } from 'bun:test'
import { act, cleanup, render } from '@testing-library/react'
import { Profiler } from 'react'

mock.module('lucide-react', () => new Proxy({}, { get: () => () => null }))

import { AgentView } from '@/components/AgentView'
import { useAgentStore } from '@/stores/useAgentStore'
import type { ContentBlock } from '@/api/types'

const label = process.env.OAD_LABEL ?? 'tree'
afterEach(cleanup)

const ANSWER = [
  'Here is what I changed in the billing module:',
  '',
  '- rounding now happens once, at the invoice level',
  '- tax lines keep **four** decimals until the total',
  '',
  '```rust',
  'fn round_invoice(x: f64) -> f64 {',
  '    (x * 100.0).round() / 100.0',
  '}',
  '```',
  '',
  'The tests in `billing/tests.rs` cover the edge cases.',
].join('\n')

function finishedTurns(n: number): ContentBlock[] {
  const out: ContentBlock[] = []
  for (let i = 0; i < n; i++) {
    out.push({ id: `u${i}`, type: 'user', content: `Prompt ${i}: continue with the next step` })
    for (let k = 0; k < 3; k++) {
      out.push({ id: `t${i}-${k}`, type: 'tool', content: '', toolName: 'read', toolCallId: `t${i}-${k}`, toolArgs: '{"path":"src/billing/mod.rs"}', toolDone: true, toolResult: 'fn main() {}\n'.repeat(20) })
    }
    out.push({ id: `a${i}`, type: 'text', content: ANSWER })
  }
  return out
}

it('transcript render while streaming', () => {
  useAgentStore.setState({ sessionId: 'bench-sid' })
  const blocks = finishedTurns(80)
  const prompt: ContentBlock = { id: 'live-u', type: 'user', content: 'Now refactor the tax code' }
  let commits = 0
  let reactMs = 0
  const onRender = (_id: string, _phase: string, actual: number) => {
    commits += 1
    reactMs += actual
  }
  const view = (current: ContentBlock[]) => (
    <Profiler id="view" onRender={onRender}>
      <AgentView blocks={blocks} currentBlocks={current} isWorking />
    </Profiler>
  )
  const mountStart = performance.now()
  const { rerender } = render(view([prompt]))
  const mountMs = performance.now() - mountStart
  commits = 0
  reactMs = 0

  let text = ''
  const tools: ContentBlock[] = []
  const start = performance.now()
  for (let f = 0; f < 300; f++) {
    if (f > 0 && f % 30 === 0) {
      tools.push({ id: `live-t${f}`, type: 'tool', content: '', toolName: 'shell', toolCallId: `live-t${f}`, toolArgs: '{"command":"cargo test"}', toolDone: true, toolResult: 'ok' })
      text = ''
    }
    text += 'stream the text '
    act(() => rerender(view([prompt, ...tools, { id: `live-text-${tools.length}`, type: 'text', content: text }])))
  }
  const streamMs = performance.now() - start
  console.log(`BENCH [${label}] web_render_mount_80_turns value_ms=${mountMs.toFixed(0)}`)
  console.log(`BENCH [${label}] web_render_300_flushes wall_ms=${streamMs.toFixed(0)} react_ms=${reactMs.toFixed(0)} commits=${commits}`)
})
