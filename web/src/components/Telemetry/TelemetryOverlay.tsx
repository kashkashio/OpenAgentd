/**
 * TelemetryOverlay — usage, spend, and turn traces as a Settings-style
 * overlay mounted at the app root, so the status bar, the mobile drawer, the
 * command palette, and ``/telemetry`` deep links all open the same surface
 * without leaving the current route.
 *
 * Escape steps back from a trace before it closes the overlay.
 */
import { useCallback } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { AnimatePresence, motion } from 'framer-motion'
import { ArrowLeft, X } from 'lucide-react'

import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useModalFocus } from '@/hooks/useModalFocus'
import { useReducedMotion } from '@/hooks/useReducedMotion'
import { DURATIONS_S, EASINGS } from '@/lib/motion'
import { cn } from '@/lib/utils'
import { useTelemetryStore } from '@/stores/useTelemetryStore'
import { useUIStore } from '@/stores/useUIStore'
import { TelemetryView } from './TelemetryView'

/** Mirrors SettingsModal's panel motion; reduced motion keeps only the fade. */
const PANEL_VARIANTS = {
  hidden: { opacity: 0, scale: 0.98, y: 4 },
  visible: { opacity: 1, scale: 1, y: 0 },
} as const
const PANEL_VARIANTS_REDUCED = {
  hidden: { opacity: 0 },
  visible: { opacity: 1 },
} as const

const HEADER_BUTTON_CLASS =
  'flex h-9 w-9 shrink-0 items-center justify-center rounded-sm text-(--color-text-muted) transition-colors hover:bg-(--bg-key) hover:text-(--color-text) focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-(--focus-ring) md:h-7 md:w-7'

export function TelemetryOverlay() {
  const open = useUIStore((s) => s.telemetryOpen)
  const close = useUIStore((s) => s.closeTelemetry)
  const traceId = useTelemetryStore((s) => s.traceId)
  const closeTrace = useTelemetryStore((s) => s.closeTrace)
  const prefersReducedMotion = useReducedMotion()
  const panel = prefersReducedMotion ? PANEL_VARIANTS_REDUCED : PANEL_VARIANTS
  const navigate = useNavigate()
  const openSession = useCallback(
    (sessionId: string) => {
      close()
      void navigate({ to: '/$sessionId', params: { sessionId } })
    },
    [close, navigate],
  )

  useModalFocus(open, () => {
    if (useTelemetryStore.getState().traceId) closeTrace()
    else close()
  }, undefined, { kind: 'overlay' })

  return (
    <AnimatePresence>
      {open && (
        <>
          <motion.div
            key="telemetry-backdrop"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: DURATIONS_S.fast }}
            className="fixed inset-0 z-50 bg-black/40"
            onClick={close}
            aria-hidden="true"
            data-swipe-ignore
          />
          <motion.div
            key="telemetry-panel"
            role="dialog"
            aria-modal="true"
            aria-label="Telemetry"
            data-modal-focus="true"
            data-swipe-ignore
            initial={panel.hidden}
            animate={panel.visible}
            exit={panel.hidden}
            transition={{ duration: prefersReducedMotion ? 0 : DURATIONS_S.fast, ease: EASINGS.out }}
            className={cn(
              'settings-modal-shell z-50 flex flex-col overflow-hidden rounded-lg',
              'border border-(--color-border) bg-(--bg-page) shadow-2xl',
            )}
          >
            <div className="flex h-11 shrink-0 items-center justify-between gap-2 border-b border-(--color-border) bg-(--bg-sidebar) px-2 select-none sm:px-4">
              <div className="flex min-w-0 items-center gap-1.5">
                {traceId && (
                  <Tooltip>
                    <TooltipTrigger
                      render={
                        <button type="button" onClick={closeTrace} className={cn(HEADER_BUTTON_CLASS, '-ml-1')} aria-label="Back to overview">
                          <ArrowLeft size={14} aria-hidden="true" />
                        </button>
                      }
                    />
                    <TooltipContent>Back to overview (Esc)</TooltipContent>
                  </Tooltip>
                )}
                <h2 className="truncate text-base font-semibold text-(--color-text)">
                  {traceId ? 'Turn trace' : 'Telemetry'}
                </h2>
              </div>
              <Tooltip>
                <TooltipTrigger
                  render={
                    <button type="button" onClick={close} className={HEADER_BUTTON_CLASS} aria-label="Close telemetry">
                      <X size={14} aria-hidden="true" />
                    </button>
                  }
                />
                <TooltipContent>{traceId ? 'Close' : 'Close (Esc)'}</TooltipContent>
              </Tooltip>
            </div>
            <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
              <TelemetryView onOpenSession={openSession} />
            </div>
          </motion.div>
        </>
      )}
    </AnimatePresence>
  )
}
