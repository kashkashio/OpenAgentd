/**
 * The CSS Custom Highlight API, where the browser has it (Chrome 105+,
 * Safari 17.2+). Highlights paint ``Range``s without touching the DOM, which
 * is what lets features mark text that React owns: wrapping it in elements
 * would replace nodes React still writes to. Without the API, callers simply
 * paint nothing.
 */
export interface HighlightRegistry {
  set(name: string, highlight: unknown): void
  delete(name: string): void
}

export function highlightApi(): { registry: HighlightRegistry; create: (ranges: Range[]) => unknown } | null {
  const registry = (globalThis as { CSS?: { highlights?: HighlightRegistry } }).CSS?.highlights
  const Ctor = (globalThis as { Highlight?: new (...ranges: Range[]) => unknown }).Highlight
  if (!registry || !Ctor) return null
  return { registry, create: (ranges) => new Ctor(...ranges) }
}
