/**
 * Per-project settings (`<workspace>/.openagentd/settings.yaml`): the default
 * model new sessions in the workspace start on, and Claude Code options.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { getWorkspaceSettings, putWorkspaceSettings } from '@/api/client'
import type { WorkspaceSettings, WorkspaceSettingsUpdate } from '@/api/types'
import { useAgentStore } from '@/stores/useAgentStore'
import { sameWorkspacePath } from '@/utils/workspace'
import { queryKeys } from './keys'

export function useWorkspaceSettingsQuery(workspace: string | null | undefined, enabled = true) {
  return useQuery({
    queryKey: queryKeys.coding.settings(workspace ?? ''),
    queryFn: () => getWorkspaceSettings(workspace as string),
    enabled: enabled && Boolean(workspace),
    staleTime: 30_000,
  })
}

export function useUpdateWorkspaceSettings() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (update: WorkspaceSettingsUpdate) => putWorkspaceSettings(update),
    onSuccess: (data: WorkspaceSettings, update) => {
      queryClient.setQueryData(queryKeys.coding.settings(update.workspace), data)
      // The open session was switched on the server too; mirror it locally or
      // its next message would send the old model and switch it back.
      const state = useAgentStore.getState()
      if (update.apply_to_sessions && data.model && sameWorkspacePath(state._workspace, update.workspace)) {
        state.setSessionModelSettings(data.model, data.thinking_level)
      }
    },
  })
}
