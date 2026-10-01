/**
 * Performance regression: a streamed delta must not touch the whole session.
 *
 * Deltas are applied inside an Immer recipe. Spreading a draft array, or
 * scanning it with ``find``/``some``, reads every element through the proxy,
 * and Immer then creates a child draft for each one. Measured on 2000 blocks
 * that cost ~1.5 ms per delta batch and ~0.8 ms per confirmed-card lookup, so
 * a long session paid it on every 16 ms flush and every tool event.
 *
 * Drafts are counted through ``Proxy.revocable``, which Immer uses for every
 * one, so the guard is deterministic rather than timing-based.
 */
import { describe, it, expect, beforeEach } from "bun:test"
import { useAgentStore } from "@/stores/useAgentStore"
import type { ContentBlock } from "@/api/types"

const CONFIRMED = 2000
const LIVE = 300

function countDrafts(fn: () => void): number {
  const real = Proxy.revocable
  let drafts = 0
  Proxy.revocable = ((target: object, handler: ProxyHandler<object>) => {
    drafts += 1
    return real(target, handler)
  }) as typeof Proxy.revocable
  try {
    fn()
  } finally {
    Proxy.revocable = real
  }
  return drafts
}

function confirmedBlocks(): ContentBlock[] {
  const blocks: ContentBlock[] = []
  for (let i = 0; i < CONFIRMED - 1; i++) {
    blocks.push({ id: `c${i}`, type: "text", content: `confirmed ${i}`, extra: { model: "m" } })
  }
  // A tool reconciled into the confirmed rows mid-turn, still running.
  blocks.push({ id: "tc-confirmed", type: "tool", content: "", toolName: "shell", toolCallId: "tc-confirmed", toolOutput: "start\n" })
  return blocks
}

function liveBlocks(): ContentBlock[] {
  const blocks: ContentBlock[] = []
  for (let i = 0; i < LIVE - 1; i++) {
    blocks.push(
      i % 2
        ? { id: `l${i}`, type: "tool", content: "", toolName: "read", toolCallId: `tc-${i}`, toolDone: true, toolResult: "ok" }
        : { id: `l${i}`, type: "text", content: `live ${i}` },
    )
  }
  blocks.push({ id: "live-tail", type: "text", content: "answer so far" })
  return blocks
}

beforeEach(() => {
  useAgentStore.setState({
    sessionId: "perf-sid",
    leadName: "lead",
    agentStreams: {
      lead: {
        blocks: confirmedBlocks(),
        currentBlocks: liveBlocks(),
        status: "working",
        usage: { promptTokens: 0, completionTokens: 0, totalTokens: 0, cachedTokens: 0 },
        model: null,
        lastError: null,
        currentText: "",
        currentThinking: "",
        _replayPending: { message: false, thinking: false },
      },
    },
    cacheInvalidations: [],
  } as never)
  // Settle into a frozen, Immer-produced state like the live store.
  useAgentStore.getState()._handleSSEEvent("message", { agent: "lead", text: "" })
})

describe("streamed deltas stay off the rest of the session", () => {
  it("appends a text delta without drafting every live block", () => {
    const drafts = countDrafts(() => {
      useAgentStore.getState()._handleSSEEvent("message", { agent: "lead", text: " and more" })
    })
    const live = useAgentStore.getState().agentStreams.lead.currentBlocks
    expect(live[live.length - 1].content).toBe("answer so far and more")
    expect(live).toHaveLength(LIVE)
    expect(drafts).toBeLessThan(20)
  })

  it("routes output for a confirmed tool card without drafting the session", () => {
    const drafts = countDrafts(() => {
      useAgentStore.getState()._handleSSEEvent("tool_output_delta", {
        agent: "lead",
        name: "shell",
        tool_call_id: "tc-confirmed",
        text: "next line\n",
      })
    })
    const confirmed = useAgentStore.getState().agentStreams.lead.blocks
    expect(confirmed[confirmed.length - 1].toolOutput).toBe("start\nnext line\n")
    expect(drafts).toBeLessThan(20)
  })

  it("runs a tool call's lifecycle without drafting the live turn", () => {
    const state = () => useAgentStore.getState()
    const counts = [
      countDrafts(() => state()._handleSSEEvent("tool_call", { agent: "lead", name: "read", tool_call_id: "tc-new" })),
      countDrafts(() => state()._handleSSEEvent("tool_start", { agent: "lead", name: "read", tool_call_id: "tc-new", arguments: "{\"path\":\"a\"}" })),
      countDrafts(() => state()._handleSSEEvent("tool_end", { agent: "lead", name: "read", tool_call_id: "tc-new", result: "done" })),
    ]
    const live = state().agentStreams.lead.currentBlocks
    expect(live).toHaveLength(LIVE + 1)
    expect(live[live.length - 1]).toMatchObject({ toolCallId: "tc-new", toolArgs: "{\"path\":\"a\"}", toolDone: true, toolResult: "done" })
    for (const drafts of counts) expect(drafts).toBeLessThan(20)
  })

  it("finishes a confirmed tool card without drafting the session", () => {
    const drafts = countDrafts(() => {
      useAgentStore.getState()._handleSSEEvent("tool_end", { agent: "lead", name: "shell", tool_call_id: "tc-confirmed", result: "exit 0" })
    })
    const confirmed = useAgentStore.getState().agentStreams.lead.blocks
    expect(confirmed[confirmed.length - 1]).toMatchObject({ toolDone: true, toolResult: "exit 0" })
    expect(drafts).toBeLessThan(20)
  })
})
