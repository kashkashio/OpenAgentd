/**
 * Drag dock tabs to reorder them, editor style.
 *
 * Handlers go on the scrolling strip and find the tab under the pointer by
 * ``data-dock-tab``, so every tab kind (terminals included) moves the same
 * way. A press becomes a drag after 4 px, so a click still activates and a
 * middle-click still closes. The strip reorders live as the pointer crosses
 * a neighbour's midpoint, and scrolls while the pointer sits near an edge.
 * Mouse and pen only: touch keeps the strip's native scrolling.
 */
import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'

const DRAG_THRESHOLD_PX = 4
const EDGE_PX = 24
const EDGE_SCROLL_PX = 8

export const DOCK_TAB_ATTR = 'data-dock-tab'

interface DragState {
  id: string
  pointerId: number
  startX: number
  x: number
  active: boolean
}

function tabNodes(strip: HTMLElement): HTMLElement[] {
  return Array.from(strip.querySelectorAll<HTMLElement>(`[${DOCK_TAB_ATTR}]`))
}

/** Where the dragged tab belongs for a pointer at ``x``, or ``null`` to stay. */
export function dropIndex(rects: readonly { left: number; right: number }[], from: number, x: number): number | null {
  const mid = (i: number) => (rects[i].left + rects[i].right) / 2
  // Past a neighbour's midpoint, the tab takes its place.
  let to = from
  for (let i = from + 1; i < rects.length; i++) if (x > mid(i)) to = i
  if (to === from) for (let i = from - 1; i >= 0; i--) if (x < mid(i)) to = i
  return to === from ? null : to
}

export function useTabDrag({ enabled, onMove }: { enabled: boolean; onMove: (id: string, toIndex: number) => void }) {
  const dragRef = useRef<DragState | null>(null)
  const stripRef = useRef<HTMLElement | null>(null)
  const frameRef = useRef(0)
  const [draggingId, setDraggingId] = useState<string | null>(null)

  const stop = useCallback(() => {
    dragRef.current = null
    cancelAnimationFrame(frameRef.current)
    frameRef.current = 0
    setDraggingId(null)
  }, [])
  useEffect(() => () => cancelAnimationFrame(frameRef.current), [])

  const place = useCallback(() => {
    const drag = dragRef.current
    const strip = stripRef.current
    if (!drag?.active || !strip) return
    const nodes = tabNodes(strip)
    const from = nodes.findIndex((node) => node.getAttribute(DOCK_TAB_ATTR) === drag.id)
    if (from < 0) return
    const to = dropIndex(nodes.map((node) => node.getBoundingClientRect()), from, drag.x)
    if (to !== null) onMove(drag.id, to)
  }, [onMove])

  // Near an edge, keep scrolling (and re-placing) while the pointer rests.
  const edgeScroll = useCallback(() => {
    frameRef.current = 0
    const drag = dragRef.current
    const strip = stripRef.current
    if (!drag?.active || !strip) return
    const box = strip.getBoundingClientRect()
    const step = drag.x < box.left + EDGE_PX ? -EDGE_SCROLL_PX : drag.x > box.right - EDGE_PX ? EDGE_SCROLL_PX : 0
    if (step === 0) return
    strip.scrollLeft += step
    place()
    frameRef.current = requestAnimationFrame(edgeScroll)
  }, [place])

  const onPointerDown = (event: ReactPointerEvent<HTMLElement>) => {
    if (!enabled || event.button !== 0 || event.pointerType === 'touch') return
    const target = event.target as Element
    // The close button is a click target, not a handle.
    if (target.closest('[data-zone-skip]')) return
    const id = target.closest(`[${DOCK_TAB_ATTR}]`)?.getAttribute(DOCK_TAB_ATTR)
    if (!id) return
    stripRef.current = event.currentTarget
    dragRef.current = { id, pointerId: event.pointerId, startX: event.clientX, x: event.clientX, active: false }
  }

  const onPointerMove = (event: ReactPointerEvent<HTMLElement>) => {
    const drag = dragRef.current
    if (!drag || drag.pointerId !== event.pointerId) return
    drag.x = event.clientX
    if (!drag.active) {
      if (Math.abs(event.clientX - drag.startX) < DRAG_THRESHOLD_PX) return
      drag.active = true
      event.currentTarget.setPointerCapture?.(event.pointerId)
      setDraggingId(drag.id)
    }
    place()
    if (!frameRef.current) frameRef.current = requestAnimationFrame(edgeScroll)
  }

  const onPointerEnd = (event: ReactPointerEvent<HTMLElement>) => {
    if (dragRef.current?.pointerId !== event.pointerId) return
    stop()
  }

  return {
    draggingId,
    handlers: { onPointerDown, onPointerMove, onPointerUp: onPointerEnd, onPointerCancel: onPointerEnd },
  }
}
