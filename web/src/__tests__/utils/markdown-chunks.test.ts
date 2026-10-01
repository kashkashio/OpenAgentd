/**
 * `createMarkdownChunker` lets a long streaming answer re-parse only its live
 * tail. The chunks must parse to exactly the blocks the whole text parses
 * to, so the fuzz below compares against the real parser and extensions.
 */
import { describe, expect, it } from 'bun:test'
import { createMarkdownChunker, MIN_CHUNK_CHARS, type MarkdownChunk } from '@/utils/markdown-chunks'
import { parseMarkdownText } from '@/utils/markdown'

const blocksOf = (chunks: MarkdownChunk[]) => JSON.stringify(chunks.flatMap((c) => c.doc.children))
const wholeBlocks = (text: string) => JSON.stringify(parseMarkdownText(text, true).children)

function longAnswer(target: number): string {
  const parts: string[] = []
  for (let i = 0; parts.join('\n').length < target; i++) {
    parts.push(`## Step ${i}`, '', `The \`round_${i}\` helper keeps **four** decimals until the total.`, '',
      '- read it', '- test it', '', '```ts', `export const total${i} = 1`, '', 'const after = 2', '```', '',
      '| file | change |', '| --- | --- |', `| b/${i}.rs | r |`, '')
  }
  return parts.join('\n')
}

describe('createMarkdownChunker', () => {
  it('splits a long answer into settled chunks and a short live tail', () => {
    const text = longAnswer(60_000)
    const chunks = createMarkdownChunker(parseMarkdownText)(text)!
    expect(chunks.map((c) => c.text).join('')).toBe(text)
    expect(chunks.length).toBeGreaterThan(10)
    for (const c of chunks.slice(0, -1)) expect(c.text.length).toBeGreaterThanOrEqual(MIN_CHUNK_CHARS)
    expect(chunks.at(-1)!.text.length).toBeLessThan(MIN_CHUNK_CHARS * 2)
    expect(blocksOf(chunks)).toBe(wholeBlocks(text))
  })

  it('keeps settled chunks and their parsed documents as the answer grows', () => {
    const text = longAnswer(30_000)
    const chunk = createMarkdownChunker(parseMarkdownText)
    const before = chunk(text.slice(0, 20_000))!
    const after = chunk(text)!
    for (let i = 0; i < before.length - 1; i++) expect(after[i]).toBe(before[i])
  })

  it('does not split a list, a fence or a math block at a blank line', () => {
    const pad = 'p'.repeat(10) + '\n\n'
    for (const body of [
      '- one\n\n- two\n\n  still two\n\n',
      '```ts\nconst a = 1\n\nconst b = 2\n```\n\n',
      '~~~\nx\n\ny\n~~~\n\n',
      '$$\na\n\nb\n$$\n\n',
    ]) {
      const chunks = createMarkdownChunker(parseMarkdownText, 0)(`${pad}${body}after\n`)!
      expect(chunks.at(-1)!.text).toBe('after\n')
      let offset = 0
      for (const c of chunks.slice(0, -1)) {
        offset += c.text.length
        expect(offset <= pad.length || offset >= pad.length + body.length).toBe(true)
      }
    }
  })

  it('renders whole when the answer uses document-wide markdown', () => {
    const pad = 'x\n\n'.repeat(2000)
    for (const tail of ['See [docs].\n\n[docs]: https://example.com', 'A note.[^1]\n\n[^1]: the note', '<proposed_plan>\n# Plan\n</proposed_plan>', '<!-- a\n\nb -->']) {
      expect(createMarkdownChunker(parseMarkdownText)(pad + tail)).toBeNull()
    }
  })

  it('renders whole when the finished text no longer parses like its chunks', () => {
    const text = 'first paragraph\n\nsecond paragraph\n'
    // Stands in for later text changing how earlier text parses.
    let changed = false
    const parse = (t: string, live: boolean) => parseMarkdownText(changed && t === text ? 'changed' : t, live)
    const chunk = createMarkdownChunker(parse, 0)
    expect(chunk(text)!.length).toBe(2)
    changed = true
    expect(chunk(text, true)).toBeNull()
  })

  it('does not cut after a math marker the parser left as text', () => {
    // The first `$$` continues the list item, so the line scan pairs it with
    // the second. For the parser that second `$$` is an opener with no closer
    // yet (plain text); a later `$$` would make it math past any cut after it.
    const text = '- item\n$$\n\n$$\n\nNext paragraph.\n\nAnother.\n'
    expect(createMarkdownChunker(parseMarkdownText, 0)(text)!.map((c) => c.text)).toEqual([text])
    // Closed display math is settled, so it does not stop later cuts.
    for (const math of ['$$\nx\n$$', '\\[\nx\n\\]']) {
      const closed = `Intro.\n\n${math}\n\nNext paragraph.\n\nAnother.\n`
      expect(createMarkdownChunker(parseMarkdownText, 0)(closed)!.map((c) => c.text)).toEqual(['Intro.\n\n', `${math}\n\n`, 'Next paragraph.\n\n', 'Another.\n'])
    }
  })
})

// Seeded LCG so failures reproduce.
function rng(seed: number) {
  let s = seed >>> 0
  return () => (s = (s * 1664525 + 1013904223) >>> 0) / 2 ** 32
}

const LINES = [
  'Plain text with `code` and **bold**.', 'More prose here.', 'text with trailing spaces  ',
  '# Heading', '#', '## ', 'Setext title', '===', '---', '***', '- - -', '___',
  '- item', '* item', '+ item', '1. one', '2) two', '10. ten', '-', '1.', '- [ ] task',
  '  continued in item', '   three spaces', '    indented code', '\tTab indented',
  '> quote', '>', '> [!NOTE]', '> - quoted list', '> ```',
  '```', '```ts', '```markdown', '````', '````python', '~~~', '~~~js', '  ```', '   ~~~', '    ```', '``` ```',
  '```mermaid', 'graph TD; A-->B',
  '$$', '$$x^2$$', '$$ a + b', 'c $$', '\\[', '\\]', '$$x$$ inline after',
  '| a | b |', '| --- | --- |', '| 1 | 2 |',
]

function fuzzDoc(next: () => number): string {
  const n = 5 + Math.floor(next() * 40)
  let doc = ''
  for (let i = 0; i < n; i++) {
    const r = next()
    doc += LINES[Math.floor(next() * LINES.length)] + (r < 0.55 ? '\n' : r < 0.9 ? '\n\n' : '\n\n\n')
  }
  return doc
}

const fail = (text: string, chunks: MarkdownChunk[]) =>
  new Error(`chunked parse differs for ${JSON.stringify(text)}\nchunks: ${JSON.stringify(chunks.map((c) => c.text))}`)

describe('createMarkdownChunker equivalence', () => {
  it('chunks parse to the same blocks as the whole text, at every prefix', () => {
    const next = rng(20261001)
    let split = 0
    for (let d = 0; d < 1500; d++) {
      const doc = fuzzDoc(next)
      for (const cut of [doc.length, ...Array.from({ length: 4 }, () => Math.floor(next() * doc.length))]) {
        const text = doc.slice(0, cut)
        const chunks = createMarkdownChunker(parseMarkdownText, 0)(text)
        if (!chunks) continue
        expect(chunks.map((c) => c.text).join('')).toBe(text)
        if (chunks.length > 1) split++
        if (blocksOf(chunks) !== wholeBlocks(text)) throw fail(text, chunks)
      }
    }
    expect(split).toBeGreaterThan(1000)
  })

  it('stays equivalent while one chunker follows a growing stream', () => {
    const next = rng(7)
    for (let d = 0; d < 800; d++) {
      const doc = fuzzDoc(next)
      const chunk = createMarkdownChunker(parseMarkdownText, 0)
      const steps = Array.from({ length: 8 }, () => Math.floor(next() * doc.length)).sort((a, b) => a - b)
      for (const cut of [...steps, doc.length]) {
        const text = doc.slice(0, cut)
        const chunks = chunk(text)
        if (chunks && blocksOf(chunks) !== wholeBlocks(text)) throw fail(text, chunks)
      }
      const final = chunk(doc, true)
      if (final) expect(blocksOf(final)).toBe(wholeBlocks(doc))
    }
  })
})
