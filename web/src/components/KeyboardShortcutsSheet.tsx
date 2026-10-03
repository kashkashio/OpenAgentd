/**
 * Keyboard Shortcuts — every shortcut, grouped by where it works.
 *
 * Opened with ⌘/ (Ctrl+/) or from the command palette. It is one of the
 * app's overlays (``useUIStore``), so ⌘K / ⌘P / ⌘, swap it for another one.
 */
import { Keyboard } from 'lucide-react'

import { AppOverlay } from '@/components/ui/app-overlay'
import { SectionCard, SectionCardHeader, SectionCardRow, SectionCardRows } from '@/components/ui/section-card'
import { usePlatform } from '@/hooks/use-platform'
import { shortcutHelp } from '@/lib/keyboard/help'
import { useUIStore } from '@/stores/useUIStore'

export function KeyboardShortcutsSheet() {
  const open = useUIStore((s) => s.shortcutsHelpOpen)
  const close = useUIStore((s) => s.closeShortcutsHelp)
  const { os, isTauri } = usePlatform()

  return (
    <AppOverlay open={open} onClose={close} label="Keyboard shortcuts" maxWidth="640px">
      <div className="flex h-11 shrink-0 items-center gap-2 border-b border-(--color-border) bg-(--bg-sidebar) px-4 select-none">
        <Keyboard size={14} className="shrink-0 text-(--color-text-muted)" aria-hidden="true" />
        <h2 className="text-base font-semibold text-(--color-text)">Keyboard shortcuts</h2>
      </div>
      <div className="relative min-h-0 flex-1 space-y-4 overflow-y-auto overscroll-contain touch-pan-y px-5 py-4">
        <p className="text-sm text-(--color-text-muted)">
          Escape closes whatever is on top. While a dialog is open, app shortcuts wait; ⌘K, ⌘P and ⌘, can still switch to another overlay.
        </p>
        {shortcutHelp(os, { desktopApp: isTauri }).map((group) => (
          <SectionCard key={group.title}>
            <SectionCardHeader>{group.title}</SectionCardHeader>
            <SectionCardRows>
              {group.entries.map((entry) => (
                <SectionCardRow key={entry.label}>
                  <span className="min-w-0 flex-1 text-sm text-(--color-text)">{entry.label}</span>
                  <span className="flex shrink-0 items-center gap-1">
                    {entry.keys.map((key) => (
                      <kbd
                        key={key}
                        className="rounded-xs border border-(--color-border) bg-(--bg-card) px-1.5 py-0.5 font-mono text-xs text-(--color-text-muted)"
                      >
                        {key}
                      </kbd>
                    ))}
                  </span>
                </SectionCardRow>
              ))}
            </SectionCardRows>
          </SectionCard>
        ))}
      </div>
    </AppOverlay>
  )
}
