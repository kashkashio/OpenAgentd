/**
 * Splits a streaming markdown answer into settled chunks and a live tail,
 * so each update re-parses only the tail instead of the whole text (8 ms per
 * frame at 100k characters before). The chunks must render exactly what the
 * whole text renders.
 *
 * A line scan proposes cuts: before a complete, unindented line after a blank
 * line, outside fences and math blocks, that no block above can continue.
 * The parser has quirks a scan can't fully mirror, so the parser decides:
 * a cut is taken only if the head and the rest parse to the same blocks as
 * the tail they came from. Those are the parses rendering needs anyway, so
 * a check costs about one extra tail parse per cut. Text arriving later can
 * still change how earlier text parses (a math block with no closer yet is
 * a paragraph), so the final text is checked against one whole parse.
 */
import type { parseMarkdown } from '@tanstack/markdown'

type MarkdownDocument = ReturnType<typeof parseMarkdown>
/** Parses `text`; `live` adds the trailing-placeholder trim of a stream tail. */
export type ParseMarkdownText = (text: string, live: boolean) => MarkdownDocument
export interface MarkdownChunk {
  text: string
  doc: MarkdownDocument
}

/** Smallest settled chunk, so a long answer is tens of chunks, not thousands. */
export const MIN_CHUNK_CHARS = 2000

// Document-wide constructs (reference definitions, footnotes, HTML blocks,
// plan tags): text using them is rendered whole.
const WHOLE_DOCUMENT = /^ {0,3}(?:\[[^\]\n]+\]:|<[A-Za-z!?/])|proposed_plan/im
// Lines that can continue the block above a blank line. Unlike CommonMark,
// the parser also joins quotes separated by a blank line.
const CONTINUES_BLOCK = /^(?:(?:[-+*]|\d{1,9}[.)])(?:[ \t]|$)|>)/
const FENCE = /^ {0,3}(`{3,}|~{3,})(.*)$/
const FENCE_OR_MATH = /```|~~~|\$\$|\\\[/

/** Offsets in `text` where a cut may go, by the line scan alone. */
function proposedCuts(text: string): number[] {
  const cuts: number[] = []
  let fence: { char: string; length: number } | null = null
  let mathClose: string | null = null
  let previousBlank = false
  // A fence or math marker inside a quote or list item: the parser reads the
  // blank lines after it differently depending on what follows, so skip the
  // next cut. That cut line closes every container.
  let containerFence = false
  // Only complete lines: the line still being written can't be a cut, and
  // its first characters may yet turn into a list marker.
  for (let pos = 0, end = text.indexOf('\n'); end !== -1; pos = end + 1, end = text.indexOf('\n', pos)) {
    const line = text.slice(pos, end)
    const trimmed = line.trim()
    if (fence) {
      const close = FENCE.exec(line)
      if (close && close[1][0] === fence.char && close[1].length >= fence.length && close[2].trim() === '') fence = null
    } else if (mathClose) {
      if (trimmed.endsWith(mathClose)) mathClose = null
    } else {
      if (previousBlank && pos > 0 && /^\S/.test(line) && !CONTINUES_BLOCK.test(line)) {
        if (!containerFence) cuts.push(pos)
        containerFence = false
      }
      // The parser opens a backtick fence even when its info string holds
      // backticks (```` ``` ``` ````), unlike CommonMark.
      const open = FENCE.exec(line)
      if (open) {
        fence = { char: open[1][0], length: open[1].length }
      } else if (trimmed.startsWith('$$') || trimmed.startsWith('\\[')) {
        // Mirrors parseMathBlock: single-line unless it doesn't close itself.
        const [openToken, closeToken] = trimmed.startsWith('$$') ? ['$$', '$$'] : ['\\[', '\\]']
        const closed = trimmed.length > 2 && trimmed.endsWith(closeToken) && trimmed.slice(openToken.length).includes(closeToken)
        if (!closed) mathClose = closeToken
      } else if (FENCE_OR_MATH.test(line)) {
        containerFence = true
      }
    }
    previousBlank = trimmed === ''
  }
  return cuts
}

const sameBlocks = (a: MarkdownDocument['children'], b: MarkdownDocument['children']) => JSON.stringify(a) === JSON.stringify(b)

// Closing lines of the two display-math forms.
const MATH_CLOSERS = ['$$', '\\]']

/**
 * Whether a math closer arriving later would change how `head` parses: a
 * math opener with no closer yet is plain text until one comes, then becomes
 * math reaching past any cut placed after it. Probing with the closers
 * themselves catches every such opener, escaped `\[` included.
 */
function closesLater(parse: ParseMarkdownText, head: string, doc: MarkdownDocument): boolean {
  return MATH_CLOSERS.some((closer) => !sameBlocks(parse(`${head}\n\n${closer}\n`, false).children.slice(0, doc.children.length), doc.children))
}

/**
 * One per streamed message. Call with the current text on every update; it
 * keeps the settled chunks (and their parsed documents) while the text
 * still starts with them. Returns null when the text must render whole.
 * Pass `final` once the stream has ended to check against a whole parse.
 */
export function createMarkdownChunker(parse: ParseMarkdownText, minChunk: number = MIN_CHUNK_CHARS) {
  let settled: MarkdownChunk[] = []
  let settledEnd = 0
  // Absolute offsets whose cut failed the parse check; never retried.
  const rejected = new Set<number>()
  let checked: { text: string; ok: boolean } | null = null

  return function chunk(text: string, final = false): MarkdownChunk[] | null {
    if (WHOLE_DOCUMENT.test(text)) return null
    let keep = 0
    let end = 0
    while (keep < settled.length && text.startsWith(settled[keep].text, end)) end += settled[keep++].text.length
    if (keep < settled.length) {
      settled = settled.slice(0, keep)
      settledEnd = end
      for (const at of rejected) if (at >= end) rejected.delete(at)
    }
    let tail = text.slice(settledEnd)
    let tailDoc = parse(tail, true)
    let lastCut = 0
    for (const at of proposedCuts(tail)) {
      const cut = at - lastCut
      if (cut < minChunk || rejected.has(settledEnd + cut)) continue
      const head = parse(tail.slice(0, cut), false)
      const rest = parse(tail.slice(cut), true)
      if (!sameBlocks([...head.children, ...rest.children], tailDoc.children) || closesLater(parse, tail.slice(0, cut), head)) {
        rejected.add(settledEnd + cut)
        continue
      }
      settled.push({ text: tail.slice(0, cut), doc: head })
      settledEnd += cut
      lastCut = at
      tail = tail.slice(cut)
      tailDoc = rest
    }
    const chunks = [...settled, { text: tail, doc: tailDoc }]
    if (!final || chunks.length === 1) return chunks
    if (checked?.text !== text) {
      checked = { text, ok: sameBlocks(chunks.flatMap((c) => c.doc.children), parse(text, true).children) }
    }
    return checked.ok ? chunks : null
  }
}
