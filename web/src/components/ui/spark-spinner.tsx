/**
 * SparkSpinner — the working indicator: an asterisk that grows and shrinks
 * through a few glyphs, in the accent color (the Claude Code CLI look).
 * Pure CSS (``.spark-spinner`` in index.css): the glyph is generated content,
 * so it stays out of copied text, and reduced motion leaves it still.
 */
import { cn } from '@/lib/utils'

/** ``label: null`` makes it decorative, for rows that name the state in text. */
export function SparkSpinner({ className, label = 'Working' }: { className?: string; label?: string | null }) {
  return (
    <span
      {...(label ? { role: 'img', 'aria-label': label } : { 'aria-hidden': true })}
      className={cn('spark-spinner inline-flex w-[1.1em] shrink-0 justify-center font-sans leading-none text-(--color-accent)', className)}
    />
  )
}
