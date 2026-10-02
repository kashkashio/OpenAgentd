import { isDesktopShell } from '@/lib/desktop-shell'
import { cn } from '@/lib/utils'

interface SkipLinkProps {
  href?: string
  children?: React.ReactNode
  className?: string
}

/**
 * Skip link — hidden until the first `Tab` press. Moves keyboard focus
 * past navigation directly to the main content area.
 *
 * Styling lives in `.skip-link` in index.css so the CSS transition stays
 * a single source of truth.
 *
 * The desktop app has no skip link: Tab moves between areas there (see
 * `lib/focus/zones.ts`), so there is no long run of links to skip.
 */
export function SkipLink({
  href = '#main',
  children = 'Skip to main content',
  className,
}: SkipLinkProps) {
  if (isDesktopShell()) return null
  return (
    <a href={href} className={cn('skip-link', className)}>
      {children}
    </a>
  )
}
