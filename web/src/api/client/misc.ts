/**
 * OpenAgentd API client — misc endpoints: health, agent status, URL helpers.
 */

import { setServerCapabilities } from '@/lib/server-capabilities'
import { apiBaseUrl } from '../base-url'
import { parseDetailOrThrow } from './_shared'
import { listSessionAgents } from './agent'
import type {
  AgentRegistryResponse,
  AgentStatusResponse,
} from '../types'

export interface HealthResponse {
  status: string
  version: string
  /** Optional server features (v3). Absent on v2, which has none of them. */
  capabilities?: string[]
}

export async function health(): Promise<HealthResponse> {
  const res = await fetch(`${apiBaseUrl()}/health/ready`)
  if (!res.ok) await parseDetailOrThrow(res, 'health')
  const data = (await res.json()) as HealthResponse
  setServerCapabilities((data as { capabilities?: string[] }).capabilities)
  return data
}

// ── Plugins (v3 only: `capabilities` includes "api.plugins") ────────────────

export interface PluginStatus {
  name: string
  file: string
  path: string
  status: 'loaded' | 'error'
  /** Provider id when the file registers a model provider. */
  provider: string | null
  /** Tool hooks the file registers (`tool.before`, `tool.after`). */
  hooks: string[]
  errors: string[]
}

export interface PluginsResponse {
  dirs: string[]
  plugins: PluginStatus[]
  /** v2 `*.py` plugins with no `*.ts`/`*.js` port — v3 does not run them. */
  unported: { name: string; file: string; path: string }[]
}

export async function listPlugins(): Promise<PluginsResponse> {
  const res = await fetch(`${apiBaseUrl()}/plugins`)
  if (!res.ok) await parseDetailOrThrow(res, 'listPlugins')
  return res.json()
}

// ── Agent status derived from /agent/agents ─────────────────────────────────
//
// There is no separate status endpoint: this is a projection of
// `GET /agent/agents`. It goes through `listSessionAgents` rather than fetching
// directly so the session store's call shares one round trip with the header's
// TanStack query (see the coalescing note in `client/agent.ts`).
//
// `/agent/agents` carries no per-agent run state, so `state` is always 'idle'
// here; live working/idle transitions come from the SSE `agent_status` events.

export function shapeAgentStatus(data: AgentRegistryResponse): AgentStatusResponse | null {
  const agents = data.agents ?? []
  const agent = agents[0]
  if (!agent) return null
  return {
    lead: { name: agent.name, model: agent.model ?? '', state: 'idle' },
    members: [],
  }
}

export async function agentStatus(workspace?: string | null, sessionId?: string | null): Promise<AgentStatusResponse | null> {
  if (!workspace) return null
  try {
    return shapeAgentStatus(await listSessionAgents(workspace, sessionId))
  } catch {
    // Soft failure: callers treat null as "agent mode unavailable".
    return null
  }
}
