import { useEffect, useState, type RefObject } from 'react'

const identity = (width: number) => width
const renderedWidth = (node: HTMLElement) => node.getBoundingClientRect().width

/**
 * Track a value derived from an element's rendered width, re-rendering only
 * when that value changes. A shell that needs one bit ("is the center too
 * narrow?") can pass a boolean ``select`` and skip the per-frame re-renders a
 * raw width causes during a sidebar tween or window drag.
 *
 * ``read`` replaces the rendered width, for example with the width the
 * element settles at once a neighbouring panel finishes its tween.
 *
 * Falls back to the window width when the element has no layout yet (first
 * paint, test DOMs) or ResizeObserver is unavailable, and coalesces bursts to
 * one measurement per animation frame. Pass a stable (module-level) ``select``
 * and ``read``: a new function each render re-subscribes the observer.
 */
export function useElementWidthSelect<T>(
  ref: RefObject<HTMLElement | null>,
  select: (width: number) => T,
  read: (node: HTMLElement) => number = renderedWidth,
): T {
  const [value, setValue] = useState(() => select(typeof window === 'undefined' ? 0 : window.innerWidth))

  useEffect(() => {
    const node = ref.current
    if (typeof window === 'undefined') return
    let frame: number | null = null
    const measure = () => {
      frame = null
      const measured = node ? read(node) : 0
      const next = select(Math.round(measured > 0 ? measured : window.innerWidth))
      setValue((prev) => (Object.is(prev, next) ? prev : next))
    }
    const schedule = () => {
      if (frame !== null) return
      frame = requestAnimationFrame(measure)
    }
    measure()
    let observer: ResizeObserver | null = null
    if (node && typeof ResizeObserver !== 'undefined') {
      observer = new ResizeObserver(schedule)
      observer.observe(node)
    }
    window.addEventListener('resize', schedule)
    return () => {
      observer?.disconnect()
      window.removeEventListener('resize', schedule)
      if (frame !== null) cancelAnimationFrame(frame)
    }
  }, [ref, select, read])

  return value
}

/** Track an element's rendered width (rounded px). */
export function useElementWidth(ref: RefObject<HTMLElement | null>, read?: (node: HTMLElement) => number): number {
  return useElementWidthSelect(ref, identity, read)
}
