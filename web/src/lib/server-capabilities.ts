/**
 * The capabilities the connected server advertised in its last health
 * response, as a tiny store with no React Query dependency, so any component
 * (and any test, whatever it mocks) can read it. `health()` keeps it fresh.
 */
let current: readonly string[] = []
const listeners = new Set<() => void>()

export function setServerCapabilities(next: readonly string[] | undefined): void {
  const caps = next ?? []
  if (caps.length === current.length && caps.every((c, i) => c === current[i])) return
  current = caps
  for (const l of listeners) l()
}

export function serverCapabilities(): readonly string[] {
  return current
}

export function subscribeServerCapabilities(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}
