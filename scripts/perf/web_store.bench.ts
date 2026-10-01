/**
 * Web store streaming benchmark (plan Phase 2), runnable unchanged at
 * fad35386 and HEAD. scripts/perf/run-web.sh copies it into
 * web/src/__tests__/ of a tree and runs it with `bun test`.
 *
 * Replays one long turn into a session that already holds 2000 confirmed
 * blocks: 200 tool calls, each preceded by six 16-character text flushes
 * (about 19k characters of answer), with five output chunks per tool.
 */
import { it } from 'bun:test'
import { useAgentStore } from '@/stores/useAgentStore'
import type { ContentBlock } from '@/api/types'

const label = process.env.OAD_LABEL ?? 'tree'

function seed() {
  const blocks: ContentBlock[] = []
  for (let i = 0; i < 2000; i++) {
    blocks.push(i % 3 === 0 ? { id: `u${i}`, type: 'user', content: `prompt ${i}` } : { id: `c${i}`, type: 'text', content: `confirmed answer ${i}`, extra: { model: 'm' } })
  }
  useAgentStore.setState({
    sessionId: 'bench-sid',
    leadName: 'lead',
    agentStreams: {
      lead: {
        blocks,
        currentBlocks: [],
        status: 'working',
        usage: { promptTokens: 0, completionTokens: 0, totalTokens: 0, cachedTokens: 0 },
        model: null,
        lastError: null,
        currentText: '',
        currentThinking: '',
        _replayPending: { message: false, thinking: false },
      },
    },
    cacheInvalidations: [],
  } as never)
  useAgentStore.getState()._handleSSEEvent('message', { agent: 'lead', text: '' })
}

function replay() {
  const send = useAgentStore.getState()._handleSSEEvent
  for (let t = 0; t < 200; t++) {
    for (let k = 0; k < 6; k++) send('message', { agent: 'lead', text: 'stream the text ' })
    const id = `tc-${t}`
    send('tool_call', { agent: 'lead', name: 'shell', tool_call_id: id })
    send('tool_start', { agent: 'lead', name: 'shell', tool_call_id: id, arguments: '{"command":"cargo test"}' })
    for (let k = 0; k < 5; k++) send('tool_output_delta', { agent: 'lead', name: 'shell', tool_call_id: id, text: `line ${k}\n` })
    send('tool_end', { agent: 'lead', name: 'shell', tool_call_id: id, result: 'ok' })
  }
}

function median(xs: number[]) {
  return [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)]
}

it('web store replay', () => {
  const times: number[] = []
  for (let r = 0; r < 7; r++) {
    seed()
    const t = performance.now()
    replay()
    times.push(performance.now() - t)
  }
  seed()
  const real = Proxy.revocable
  let drafts = 0
  Proxy.revocable = ((target: object, handler: ProxyHandler<object>) => {
    drafts += 1
    return real(target, handler)
  }) as typeof Proxy.revocable
  try {
    replay()
  } finally {
    Proxy.revocable = real
  }
  const live = useAgentStore.getState().agentStreams.lead.currentBlocks
  console.log(`BENCH [${label}] web_store_replay_2400_events median_ms=${median(times).toFixed(1)} min_ms=${Math.min(...times).toFixed(1)} runs=${times.length}`)
  console.log(`BENCH [${label}] web_store_replay_immer_drafts value=${drafts} live_blocks=${live.length}`)
})
