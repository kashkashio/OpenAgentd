/**
 * Regex lookbehind is a parse-time SyntaxError on Safari/WebKit before 16.4.
 * The desktop app supports macOS 11 and the mobile app iOS 15, whose WebKit
 * can be older, and one such literal in the startup chunk stops the whole app
 * from loading. Named groups (``(?<name>``) are fine; lookahead is fine.
 */
import { describe, expect, it } from 'bun:test'
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const SRC = fileURLToPath(new URL('../../', import.meta.url))

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) return name === '__tests__' ? [] : sourceFiles(path)
    return /\.(ts|tsx)$/.test(name) ? [path] : []
  })
}

describe('app source', () => {
  it('uses no regex lookbehind', () => {
    const offenders = sourceFiles(SRC).flatMap((file) =>
      readFileSync(file, 'utf8')
        .split('\n')
        .flatMap((line, i) => (/\(\?<[=!]/.test(line) ? [`${relative(SRC, file)}:${i + 1}`] : [])),
    )
    expect(offenders).toEqual([])
  })
})
