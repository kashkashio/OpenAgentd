/**
 * Whether this client can show built-in previews: the backend runs on this
 * computer, or the server lets other machines reach its previews
 * (`preview.remote`, advertised when it is LAN-exposed with an access key;
 * each preview then grants the calling machine access).
 */
import { useSyncExternalStore } from 'react'
import { isLocalBackend } from '@/api/preview'
import { serverCapabilities, subscribeServerCapabilities } from '@/lib/server-capabilities'

/** Server capability for previews reachable from other machines. */
export const REMOTE_PREVIEW_CAPABILITY = 'preview.remote'

function remotePreviewAdvertised(): boolean {
  return serverCapabilities().includes(REMOTE_PREVIEW_CAPABILITY)
}

export function usePreviewsAvailable(): boolean {
  const remote = useSyncExternalStore(subscribeServerCapabilities, remotePreviewAdvertised, () => false)
  return isLocalBackend() || remote
}
