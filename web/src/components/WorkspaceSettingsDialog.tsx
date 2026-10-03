/**
 * WorkspaceSettingsDialog — per-project defaults stored in the workspace's
 * `.openagentd/settings.yaml`.
 *
 * The model chosen here is what new sessions in the workspace start on, and
 * (unless unticked) existing sessions are switched to it too. A session can
 * still change its own model afterwards in Session settings. Each change
 * saves immediately, like Session settings.
 */
import { useState } from 'react'
import { Settings2 } from 'lucide-react'

import { AppOverlay } from '@/components/ui/app-overlay'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Dropdown, DropdownItem } from '@/components/ui/dropdown'
import { SessionModelSettings } from '@/components/SessionModelSettings'
import { useUpdateWorkspaceSettings, useWorkspaceSettingsQuery } from '@/queries/useWorkspaceSettingsQuery'
import { pathBasename } from '@/utils/workspace'

/** `claude --permission-mode` choices, with what each means here. */
export const CLAUDE_CODE_PERMISSION_MODES: Array<{ value: string; label: string; hint: string }> = [
  { value: 'acceptEdits', label: 'Accept edits', hint: 'Edits files freely; other tools follow your Claude Code allow rules.' },
  { value: 'plan', label: 'Plan only', hint: 'Reads and plans; makes no changes.' },
  { value: 'auto', label: 'Auto', hint: 'Claude Code decides which actions need approval.' },
  { value: 'dontAsk', label: "Don't ask", hint: 'Denies anything not already allowed instead of prompting.' },
  { value: 'bypassPermissions', label: 'Bypass permissions', hint: 'Runs every tool without checks. Only for trusted projects.' },
]
const DEFAULT_PERMISSION_MODE = 'acceptEdits'

export function isClaudeCodeModel(model: string | null | undefined): boolean {
  return Boolean(model?.startsWith('claude-code:'))
}

interface WorkspaceSettingsDialogProps {
  workspace: string | null
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function WorkspaceSettingsDialog({ workspace, open, onOpenChange }: WorkspaceSettingsDialogProps) {
  const settings = useWorkspaceSettingsQuery(workspace, open)
  const update = useUpdateWorkspaceSettings()
  // On by default: a project setting should cover chats started before it.
  const [applyToSessions, setApplyToSessions] = useState(true)

  if (!open || !workspace) return null

  const data = settings.data
  const model = data?.model ?? null
  const thinking = data?.thinking_level ?? null
  const permissionMode = data?.claude_code.permission_mode ?? null

  const save = (next: { model?: string | null; thinking?: string | null; permissionMode?: string | null }) => {
    const modelChange = next.model !== undefined || next.thinking !== undefined
    update.mutate({
      workspace,
      apply_to_sessions: modelChange && applyToSessions,
      model: next.model !== undefined ? next.model : model,
      thinking_level: next.thinking !== undefined ? next.thinking : thinking,
      claude_code: { permission_mode: next.permissionMode !== undefined ? next.permissionMode : permissionMode },
    })
  }

  const activeMode = CLAUDE_CODE_PERMISSION_MODES.find((m) => m.value === (permissionMode ?? DEFAULT_PERMISSION_MODE))
  const error = settings.error ?? update.error

  return (
    <AppOverlay open={open} onClose={() => onOpenChange(false)} label="Workspace settings" maxWidth="560px">
      <div className="flex h-11 shrink-0 items-center gap-2 border-b border-(--color-border) bg-(--bg-sidebar) px-4 select-none">
        <Settings2 size={14} className="shrink-0 text-(--color-text-muted)" aria-hidden="true" />
        <h2 className="truncate text-base font-semibold text-(--color-text)">
          Workspace settings
          <span className="ml-2 font-normal text-(--color-text-muted)">{pathBasename(workspace)}</span>
        </h2>
      </div>

      <div className="relative min-h-0 flex-1 overflow-y-auto overscroll-contain touch-pan-y">
        <p className="px-3 pt-3 text-xs leading-relaxed text-(--color-text-muted) sm:px-5 sm:pt-4">
          New sessions in this workspace start on this model. Pick a <span className="font-mono">claude-code</span> model to
          run sessions through your installed Claude Code CLI and its login instead of an API key.
        </p>

        {settings.isLoading ? (
          <p className="px-3 py-4 text-xs text-(--color-text-subtle) sm:px-5">Loading…</p>
        ) : (
          <>
            <SessionModelSettings
              defaultModel={null}
              sessionModel={model}
              sessionThinkingLevel={thinking}
              onChange={(nextModel, nextThinking) => save({ model: nextModel, thinking: nextThinking })}
            />
            <label className="flex items-center gap-2 px-3 pb-2 text-xs text-(--color-text-2) sm:px-5">
              <Checkbox checked={applyToSessions} onCheckedChange={setApplyToSessions} aria-label="Also switch existing sessions" />
              Also switch this workspace’s existing sessions
            </label>
            <div className="flex items-center justify-between gap-2 px-3 sm:px-5">
              <span className="text-xs text-(--color-text-subtle)">
                {!model
                  ? 'No workspace default — sessions use the agent’s model.'
                  : update.data?.sessions_updated
                    ? `Workspace default set; switched ${update.data.sessions_updated} existing session${update.data.sessions_updated === 1 ? '' : 's'}.`
                    : 'Workspace default set.'}
              </span>
              {model ? (
                <Button type="button" variant="subtle" size="xs" onClick={() => save({ model: null, thinking: null })} disabled={update.isPending}>
                  Use agent default
                </Button>
              ) : null}
            </div>

            {isClaudeCodeModel(model) ? (
              <div className="mt-4 border-t border-(--color-border-subtle) px-3 py-3 sm:px-5 sm:py-4">
                <span className="mb-1 flex h-4 items-center text-xs font-medium leading-none text-(--color-text-2)">
                  Claude Code permissions
                </span>
                <Dropdown
                  value={permissionMode ?? DEFAULT_PERMISSION_MODE}
                  onValueChange={(value) => save({ permissionMode: value === DEFAULT_PERMISSION_MODE ? null : value })}
                  trigger={activeMode?.label ?? 'Accept edits'}
                  className="min-h-9 w-full sm:w-64"
                  aria-label="Claude Code permission mode"
                >
                  {CLAUDE_CODE_PERMISSION_MODES.map((mode) => (
                    <DropdownItem key={mode.value} value={mode.value}>
                      {mode.label}
                    </DropdownItem>
                  ))}
                </Dropdown>
                <p className="mt-1.5 text-xs text-(--color-text-subtle)">{activeMode?.hint}</p>
              </div>
            ) : null}
          </>
        )}

        {error ? (
          <div className="mx-3 my-3 rounded-sm border border-(--color-error)/25 bg-(--color-error-subtle) px-3.5 py-2.5 text-xs text-(--color-error) sm:mx-5" role="alert">
            {error instanceof Error ? error.message : String(error)}
          </div>
        ) : null}

        {data?.path ? (
          <p className="px-3 pt-3 pb-4 font-mono text-xs md:text-[10px] text-(--color-text-subtle) break-all sm:px-5">{data.path}</p>
        ) : null}
      </div>

      <div className="flex justify-end gap-2 border-t border-(--color-border) bg-(--bg-sidebar) px-4 py-3 select-none">
        <Button type="button" variant="default" size="sm" onClick={() => onOpenChange(false)}>
          Done
        </Button>
      </div>
    </AppOverlay>
  )
}
