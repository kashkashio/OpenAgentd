/**
 * Built-in web preview client (`/api/preview`, v3 only).
 *
 * The backend starts a loopback listener per preview target and returns its
 * origin (`http://127.0.0.1:<port>`). The dock frames that origin, which is
 * separate from the app, so the previewed page cannot reach the app's
 * storage or access key. Previews only work when the backend runs on this
 * computer: a phone or a remote server cannot reach the listener.
 */

import { apiBaseUrl } from './base-url'
import { parseDetailOrThrow } from './client/_shared'

/** What a preview tab shows: a local dev server URL or a workspace file. */
export type PreviewTarget = { kind: 'url'; url: string } | { kind: 'file'; path: string }

export interface PreviewInfo {
  id: string
  workspace: string
  kind: 'url' | 'file'
  /** Proxied origin (`http://localhost:5173`) or the served directory. */
  target: string
  port: number
  /** Where the iframe loads from, e.g. `http://127.0.0.1:52011`. */
  origin: string
  /** First path to open (with query and hash). */
  path: string
  url: string
  console_errors: number
}

/** What a new preview opens before the workspace remembers a URL. */
export const DEFAULT_PREVIEW_URL = 'http://localhost:5173'

const LOOPBACK_HOSTS = new Set(['localhost', '127.0.0.1', '[::1]', '::1'])

export function isLoopbackHost(host: string): boolean {
  return LOOPBACK_HOSTS.has(host) || /^127\.\d+\.\d+\.\d+$/.test(host)
}

/** True when the backend runs on this computer, so its preview listeners are reachable. */
export function isLocalBackend(): boolean {
  if (typeof window === 'undefined') return false
  try {
    const base = new URL(apiBaseUrl(), window.location.href)
    if (base.protocol !== 'http:' && base.protocol !== 'https:') return false
    return isLoopbackHost(base.hostname)
  } catch {
    return false
  }
}

export function previewTargetKey(target: PreviewTarget): string {
  if (target.kind === 'file') return `file:${target.path}`
  try {
    const u = new URL(target.url.includes('://') ? target.url : `http://${target.url}`)
    return `url:${u.protocol}//${u.host}`
  } catch {
    return `url:${target.url}`
  }
}

const PORT_KEY = 'oa-preview-port:'
const LAST_URL_KEY = 'oa-preview-last-url:'

function storage(): Storage | null {
  try {
    return typeof window === 'undefined' ? null : window.localStorage
  } catch {
    return null
  }
}

function preferredPort(workspace: string, target: PreviewTarget): number | undefined {
  const key = target.kind === 'file' ? 'files' : previewTargetKey(target)
  const raw = storage()?.getItem(`${PORT_KEY}${workspace}|${key}`)
  const port = raw ? Number.parseInt(raw, 10) : Number.NaN
  return Number.isInteger(port) && port > 1024 && port < 65536 ? port : undefined
}

function rememberPort(workspace: string, target: PreviewTarget, port: number): void {
  const key = target.kind === 'file' ? 'files' : previewTargetKey(target)
  storage()?.setItem(`${PORT_KEY}${workspace}|${key}`, String(port))
}

export function lastPreviewUrl(workspace: string): string | null {
  return storage()?.getItem(`${LAST_URL_KEY}${workspace}`) ?? null
}

export function rememberPreviewUrl(workspace: string, url: string): void {
  storage()?.setItem(`${LAST_URL_KEY}${workspace}`, url)
}

/**
 * Start (or reuse) the preview for `target`. Reuses the port this target had
 * last time when it is free, so the page keeps its cookies and storage.
 */
export async function openPreview(workspace: string, target: PreviewTarget): Promise<PreviewInfo> {
  const body: Record<string, unknown> = { workspace }
  if (target.kind === 'url') body.url = target.url
  else body.path = target.path
  const port = preferredPort(workspace, target)
  if (port) body.preferred_port = port
  const res = await fetch(`${apiBaseUrl()}/preview`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) await parseDetailOrThrow(res, 'openPreview')
  const info = (await res.json()) as PreviewInfo
  rememberPort(workspace, target, info.port)
  if (target.kind === 'url') rememberPreviewUrl(workspace, target.url)
  return info
}

export async function closePreview(id: string): Promise<void> {
  const res = await fetch(`${apiBaseUrl()}/preview/${encodeURIComponent(id)}`, { method: 'DELETE' })
  if (!res.ok && res.status !== 404) await parseDetailOrThrow(res, 'closePreview')
}
