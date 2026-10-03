/**
 * Claude Code sessions (`claude-code:*` models) report the CLI's own tools:
 * `Bash`, `Read`, `Edit`, … with Claude Code's argument names. Each maps to
 * the OpenAgentd tool it corresponds to, so its row, its reader-mode count
 * and its live "Working · …" label reuse that tool's display. The row keeps
 * Claude Code's tool name as its label.
 */

/** Claude Code tool → the OpenAgentd tool whose display and count it shares. */
export const CLAUDE_CODE_TOOL_KIND: Record<string, string> = {
  Bash: 'shell',
  BashOutput: 'bg',
  KillShell: 'bg',
  Read: 'read',
  Grep: 'grep',
  Glob: 'glob',
  WebFetch: 'web_fetch',
  WebSearch: 'web_search',
  Edit: 'patch',
  MultiEdit: 'patch',
  Write: 'patch',
  NotebookEdit: 'patch',
}

/** Claude Code's file-changing tools: shown as a diff of what they wrote. */
export const CLAUDE_CODE_EDIT_TOOLS = new Set(['Edit', 'MultiEdit', 'Write', 'NotebookEdit'])

const str = (o: Record<string, unknown>, k: string) => (typeof o[k] === 'string' && o[k] ? (o[k] as string) : undefined)

/**
 * A Claude Code call as the OpenAgentd tool it corresponds to, with its
 * arguments renamed; `null` for tools without a counterpart (edits and the
 * rest get displays of their own).
 */
export function claudeCodeAsOpenAgentd(name: string, args: Record<string, unknown>): { name: string; args: Record<string, unknown> } | null {
  switch (name) {
    case 'Bash':
      return { name: 'shell', args: { command: str(args, 'command'), description: str(args, 'description') } }
    case 'Read':
      return { name: 'read', args: { path: str(args, 'file_path') } }
    case 'Grep':
      return { name: 'grep', args: { pattern: str(args, 'pattern'), directory: str(args, 'path'), include: str(args, 'glob') ?? str(args, 'type') } }
    case 'Glob':
      return { name: 'glob', args: { pattern: str(args, 'pattern'), directory: str(args, 'path') } }
    case 'WebFetch':
      return { name: 'web_fetch', args: { url: str(args, 'url') } }
    case 'WebSearch':
      return { name: 'web_search', args: { query: str(args, 'query') } }
    default:
      return null
  }
}

function diff(oldText: string, newText: string): string {
  const lines = (prefix: string, text: string) => (text ? text.split('\n').map((line) => `${prefix} ${line}`) : [])
  return [...lines('-', oldText), ...lines('+', newText)].join('\n')
}

/** The change an edit tool made, as `-`/`+` lines; `null` when the call carries none. */
export function claudeCodeEditDiff(name: string, args: Record<string, unknown>): string | null {
  if (name === 'Write') return str(args, 'content') ? diff('', str(args, 'content') as string) : null
  if (name === 'Edit') return diff(str(args, 'old_string') ?? '', str(args, 'new_string') ?? '') || null
  if (name === 'NotebookEdit') return str(args, 'new_source') ? diff('', str(args, 'new_source') as string) : null
  if (name === 'MultiEdit' && Array.isArray(args.edits)) {
    const parts = (args.edits as unknown[]).flatMap((e) => {
      if (typeof e !== 'object' || e === null) return []
      const edit = e as Record<string, unknown>
      return [diff(str(edit, 'old_string') ?? '', str(edit, 'new_string') ?? '')]
    })
    return parts.filter(Boolean).join('\n\n') || null
  }
  return null
}

/** The file an edit tool touched. */
export function claudeCodeEditPath(args: Record<string, unknown>): string | undefined {
  return str(args, 'file_path') ?? str(args, 'notebook_path')
}
