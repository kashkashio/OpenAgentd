import { highlightApi } from '@/utils/css-highlights'

/** CSS Custom Highlight names; styled in `index.css`. */
export const TRANSCRIPT_FIND_HIGHLIGHT = 'transcript-find'
export const TRANSCRIPT_FIND_ACTIVE_HIGHLIGHT = 'transcript-find-active'

/**
 * Paint every match of ``rawQuery`` in the find blocks under ``root`` and
 * return the active one's range (for scrolling), or ``null`` without a match.
 *
 * The matches are painted as CSS highlights and the DOM is left exactly as
 * React rendered it. Wrapping them in ``<mark>``s used to replace text nodes
 * React owns, so a reply streaming in with find open was written to detached
 * nodes and stopped updating, and clearing the marks re-normalized the text
 * again. Ranges go stale when that text changes, so callers repaint on DOM
 * changes. Where the browser has no highlight API, nothing is painted but the
 * active range is still returned.
 */
export function paintTranscriptFind(root: ParentNode, rawQuery: string, activeIndex: number): Range | null {
  const ranges = findRanges(root, rawQuery)
  if (ranges.length === 0) {
    clearTranscriptFind()
    return null
  }
  const active = ranges[((activeIndex % ranges.length) + ranges.length) % ranges.length]
  const api = highlightApi()
  if (api) {
    api.registry.set(TRANSCRIPT_FIND_HIGHLIGHT, api.create(ranges.filter((range) => range !== active)))
    api.registry.set(TRANSCRIPT_FIND_ACTIVE_HIGHLIGHT, api.create([active]))
  }
  return active
}

export function clearTranscriptFind(): void {
  const api = highlightApi()
  api?.registry.delete(TRANSCRIPT_FIND_HIGHLIGHT)
  api?.registry.delete(TRANSCRIPT_FIND_ACTIVE_HIGHLIGHT)
}

/** Every case-insensitive occurrence, in document order, within single text nodes. */
function findRanges(root: ParentNode, rawQuery: string): Range[] {
  const query = rawQuery.trim()
  if (!query) return []
  const needle = query.toLowerCase()
  const ranges: Range[] = []
  for (const block of root.querySelectorAll('[data-find-block]')) {
    const textNodes: Text[] = []
    collectTextNodes(block, textNodes)
    for (const textNode of textNodes) {
      const lower = (textNode.nodeValue ?? '').toLowerCase()
      let from = 0
      while (from <= lower.length - needle.length) {
        const start = lower.indexOf(needle, from)
        if (start < 0) break
        const range = document.createRange()
        range.setStart(textNode, start)
        range.setEnd(textNode, start + needle.length)
        ranges.push(range)
        from = start + needle.length
      }
    }
  }
  return ranges
}

function collectTextNodes(root: Node, out: Text[]): void {
  if (root.nodeType === Node.TEXT_NODE) {
    if (root.nodeValue) out.push(root as Text)
    return
  }
  if (root.nodeType !== Node.ELEMENT_NODE) return
  const el = root as Element
  if (el.tagName === 'SCRIPT' || el.tagName === 'STYLE') return
  for (const child of Array.from(root.childNodes)) collectTextNodes(child, out)
}
