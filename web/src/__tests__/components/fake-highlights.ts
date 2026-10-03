/**
 * A stand-in for the CSS Custom Highlight API (``CSS.highlights`` plus the
 * ``Highlight`` constructor), which happy-dom does not implement.
 */
export interface FakeHighlights {
  registry: Map<string, { ranges: Range[] }>
  /** Text of each range painted under ``name``, in order. */
  texts(name: string): string[]
  restore(): void
}

export function installFakeHighlights(): FakeHighlights {
  const scope = globalThis as unknown as { Highlight?: unknown }
  // happy-dom's ``CSS`` is a getter that builds a fresh object on every read,
  // so the fake replaces the global itself and puts the original back.
  const originalCSS = Object.getOwnPropertyDescriptor(globalThis, 'CSS')
  const registry = new Map<string, { ranges: Range[] }>()
  Object.defineProperty(globalThis, 'CSS', {
    value: { highlights: registry, supports: () => false, escape: (value: string) => value },
    configurable: true,
    writable: true,
  })
  scope.Highlight = class {
    ranges: Range[]
    constructor(...ranges: Range[]) {
      this.ranges = ranges
    }
  }
  let restored = false
  return {
    registry,
    texts: (name) => registry.get(name)?.ranges.map((range) => range.toString()) ?? [],
    restore: () => {
      if (restored) return
      restored = true
      if (originalCSS) Object.defineProperty(globalThis, 'CSS', originalCSS)
      else delete (globalThis as { CSS?: unknown }).CSS
      delete scope.Highlight
    },
  }
}
