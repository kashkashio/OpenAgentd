/**
 * Settings card: import the server's Claude Code sessions (~/.claude/projects)
 * with their workspaces. "Check" previews; "Import" writes. Safe to repeat:
 * new messages are added, and OpenAgentd's own sessions are left alone.
 */
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Download } from 'lucide-react'

import { importClaudeCode, type ClaudeCodeImportReport } from '@/api/client'
import { SettingsSection } from '@/components/settings/SettingsSection'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { queryKeys } from '@/queries/keys'

function summary(r: ClaudeCodeImportReport): string {
  const counts = r.sessions.reduce<Record<string, number>>((acc, s) => ({ ...acc, [s.status]: (acc[s.status] ?? 0) + 1 }), {})
  const parts = [
    counts.new ? `${counts.new} new` : '',
    counts.updated ? `${counts.updated} updated` : '',
    counts.unchanged ? `${counts.unchanged} up to date` : '',
    counts.skipped ? `${counts.skipped} already in OpenAgentd` : '',
    counts.error ? `${counts.error} failed` : '',
  ].filter(Boolean)
  const verb = r.dry_run ? 'Would import' : 'Imported'
  return `${parts.join(', ') || 'No sessions found'}. ${verb} ${r.messages.toLocaleString()} messages and ${r.subagents} sub-agent sessions.`
}

export function ClaudeCodeImportSection() {
  const queryClient = useQueryClient()
  const [workflows, setWorkflows] = useState(false)
  const [pending, setPending] = useState<'check' | 'import' | null>(null)
  const [report, setReport] = useState<ClaudeCodeImportReport | null>(null)
  const [error, setError] = useState<string | null>(null)

  const run = async (dryRun: boolean) => {
    setPending(dryRun ? 'check' : 'import')
    setError(null)
    try {
      const r = await importClaudeCode({ dry_run: dryRun, workflows })
      setReport(r)
      if (!dryRun) {
        await queryClient.invalidateQueries({ queryKey: queryKeys.session.sessions.all() })
        await queryClient.invalidateQueries({ queryKey: queryKeys.coding.tree() })
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setPending(null)
    }
  }

  const changed = report?.sessions.filter((s) => s.status === 'new' || s.status === 'updated') ?? []

  return (
    <SettingsSection title="Import from Claude Code">
      <div className="flex flex-wrap items-start gap-3">
        <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-xs border border-(--color-border) bg-(--bg-key) text-(--color-text-muted)" aria-hidden="true">
          <Download size={14} />
        </span>
        <div className="min-w-0 flex-1 space-y-2">
          <p className="text-xs leading-relaxed text-(--color-text-muted)">
            Bring the server's Claude Code sessions and their workspaces into OpenAgentd. Each keeps its id, so a{' '}
            <span className="font-mono">claude-code</span> model continues it. Safe to repeat: new messages are added.
          </p>
          <label className="flex items-center gap-2 text-xs text-(--color-text-2)">
            <Checkbox checked={workflows} onCheckedChange={setWorkflows} aria-label="Include workflow runs" />
            Include workflow runs (thousands of agents; slow to open)
          </label>
        </div>
        <div className="flex gap-2">
          <Button type="button" size="sm" variant="subtle" onClick={() => void run(true)} disabled={pending !== null}>
            {pending === 'check' ? 'Checking…' : 'Check'}
          </Button>
          <Button type="button" size="sm" variant="default" onClick={() => void run(false)} disabled={pending !== null}>
            {pending === 'import' ? 'Importing…' : 'Import'}
          </Button>
        </div>
      </div>
      {error ? (
        <div className="mt-3 rounded-sm border border-(--color-error)/25 bg-(--color-error-subtle) px-3 py-2 text-xs text-(--color-error)" role="alert">
          {error}
        </div>
      ) : null}
      {report ? (
        <div className="mt-3 space-y-1.5 text-xs" role="status">
          <p className="text-(--color-text-2)">{summary(report)}</p>
          {changed.length > 0 ? (
            <ul className="max-h-48 space-y-0.5 overflow-y-auto font-mono text-[11px] text-(--color-text-muted)">
              {changed.map((s) => (
                <li key={s.id} className="truncate">
                  {s.status === 'new' ? '+' : '↑'} {s.title || s.id} · {s.messages} messages{s.subagents ? ` · ${s.subagents} sub-agents` : ''}
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}
    </SettingsSection>
  )
}
