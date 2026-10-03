import { keepPreviousData, useQuery } from '@tanstack/react-query'
import { getObservabilitySummary, type ObservabilityFilters } from '@/api/client'
import { queryKeys } from './keys'

export function useObservabilitySummaryQuery(
  days: number,
  filters: ObservabilityFilters = {},
  { refetchInterval, enabled = true }: { refetchInterval?: number; enabled?: boolean } = {},
) {
  const normalized = {
    workspace: filters.workspace ?? null,
    model: filters.model ?? null,
    session: filters.session ?? null,
  }
  return useQuery({
    queryKey: queryKeys.observability.summary(days, normalized),
    queryFn: () => getObservabilitySummary(days, normalized),
    // Span aggregates evolve slowly; refresh on manual navigation only.
    staleTime: 60_000,
    refetchInterval,
    enabled,
    // Changing a filter keeps the previous numbers on screen until the new
    // window lands instead of flashing the loading skeleton.
    placeholderData: keepPreviousData,
  })
}
