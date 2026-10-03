import { describe, expect, it, beforeEach, afterEach, mock } from 'bun:test'

import { fetchTerminalTicket, handleTerminalFrame, terminalWsUrl } from '@/api/terminal'

const originalFetch = globalThis.fetch

describe('fetchTerminalTicket', () => {
  afterEach(() => {
    globalThis.fetch = originalFetch
  })

  it('posts workspace and size, returns the ticket', async () => {
    let captured: { url: string; init?: RequestInit } | null = null
    globalThis.fetch = mock(async (url: unknown, init: unknown) => {
      captured = { url: String(url), init: init as RequestInit }
      return new Response(JSON.stringify({ ticket: 'tkt_abc', expires_in: 30 }), {
        status: 200,
      })
    }) as typeof fetch

    const ticket = await fetchTerminalTicket({ workspace: '/tmp/ws' }, 40, 120)
    expect(ticket).toBe('tkt_abc')
    expect(captured!.url).toContain('/api/terminal/ticket')
    const body = JSON.parse(String(captured!.init?.body))
    expect(body).toEqual({ workspace: '/tmp/ws', rows: 40, cols: 120 })
  })

  it('throws with server detail on failure', async () => {
    globalThis.fetch = mock(async () => {
      return new Response(JSON.stringify({ detail: 'Workspace does not exist' }), {
        status: 400,
      })
    }) as typeof fetch

    await expect(fetchTerminalTicket({ workspace: '/nope' })).rejects.toThrow(
      'Workspace does not exist',
    )
  })

  it('throws generic message when body is not JSON', async () => {
    globalThis.fetch = mock(async () => new Response('oops', { status: 500 })) as typeof fetch
    await expect(fetchTerminalTicket({ workspace: '/tmp/ws' })).rejects.toThrow(
      'Terminal ticket failed (500)',
    )
  })
})

describe('terminalWsUrl', () => {
  beforeEach(() => {
    delete (window as { __OAD_API_BASE_URL__?: string }).__OAD_API_BASE_URL__
  })

  it('derives ws:// from an http origin and carries the ticket', () => {
    const url = terminalWsUrl('tkt_xyz')
    expect(url.startsWith('ws://')).toBe(true)
    expect(url).toContain('/api/terminal/ws')
    expect(url).toContain('ticket=tkt_xyz')
  })

  it('uses wss:// for an https backend base url', () => {
    ;(window as { __OAD_API_BASE_URL__?: string }).__OAD_API_BASE_URL__ =
      'https://backend.example.com'
    const url = terminalWsUrl('tkt_xyz')
    expect(url.startsWith('wss://backend.example.com/api/terminal/ws')).toBe(true)
  })

  it('url-encodes the ticket', () => {
    const url = terminalWsUrl('a b+c')
    expect(url).toContain('ticket=a%20b%2Bc')
  })
})

describe('handleTerminalFrame', () => {
  const callbacks = () => ({
    onOutput: mock(() => {}),
    onExit: mock(() => {}),
    onBusy: mock(() => {}),
    onError: mock(() => {}),
  })

  it('routes output, exit and busy frames', () => {
    const cb = callbacks()
    handleTerminalFrame(JSON.stringify({ type: 'output', data: 'hi' }), cb)
    handleTerminalFrame(JSON.stringify({ type: 'busy', busy: true }), cb)
    handleTerminalFrame(JSON.stringify({ type: 'busy', busy: false }), cb)
    handleTerminalFrame(JSON.stringify({ type: 'exit' }), cb)
    expect(cb.onOutput).toHaveBeenCalledWith('hi')
    expect(cb.onBusy.mock.calls).toEqual([[true], [false]])
    expect(cb.onExit).toHaveBeenCalledTimes(1)
    expect(cb.onError).not.toHaveBeenCalled()
  })

  it('ignores a busy frame without a boolean and reports malformed JSON', () => {
    const cb = callbacks()
    handleTerminalFrame(JSON.stringify({ type: 'busy' }), cb)
    expect(cb.onBusy).not.toHaveBeenCalled()
    handleTerminalFrame('{', cb)
    expect(cb.onError).toHaveBeenCalledTimes(1)
  })
})
