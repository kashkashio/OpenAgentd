/**
 * Focus zones: one Tab stop per area, arrow keys inside it.
 *
 * A desktop app moves Tab between areas (sidebar, chat, composer, dock,
 * status bar) and arrow keys between the controls of an area. A zone does
 * that for a container with roving ``tabIndex``: its current item gets 0,
 * every other item -1, so Tab enters the zone on the item last used and
 * leaves it in one press.
 *
 * Items are the zone's focusable descendants except:
 * - text fields (they keep their own Tab stop and arrow keys),
 * - anything under ``[data-zone-skip]``: hover-revealed row actions. The
 *   zone takes them out of Tab order (``tabIndex`` -1), so each needs a
 *   keyboard path of its own: F2, Delete, Shift+F10, a shortcut,
 * - anything under a nested zone,
 * - controls that are natively ``tabIndex=-1`` (scroll containers, inactive
 *   tabs of a tablist: only the selected tab of a tablist is an item, the
 *   tablist keeps its own arrow keys),
 * - disabled, inert, ``hidden`` or unrendered controls.
 *
 * ``tabIndex`` is set on the DOM directly: the zone cannot know every
 * component's props, and React only rewrites an attribute when its prop
 * changes. A ``MutationObserver`` re-runs the pass (at most once a frame)
 * when items appear, vanish or change state; text changes are not observed,
 * so a streaming transcript costs nothing.
 */
import { useEffect, useRef, type RefObject } from 'react'

export type ZoneOrientation = 'vertical' | 'horizontal'
export type ZoneEntry = 'first' | 'last' | 'active'

export interface FocusZoneOptions {
  orientation: ZoneOrientation
  /** Item Tab lands on before one was used: first, last, or the active one. */
  entry?: ZoneEntry
  /** Arrow keys wrap from the last item to the first. */
  wrap?: boolean
  /** ``toolbar`` sets the ARIA role and orientation. */
  role?: 'toolbar'
  label?: string
  /** Off: the zone leaves every ``tabIndex`` alone. */
  enabled?: boolean
}

const ZONE = 'data-focus-zone'
const ITEM = 'data-zone-item'
const FOCUSABLE = 'a[href], button, select, summary, input, textarea, [contenteditable="true"], [tabindex]'
const TEXT_INPUT_TYPES = new Set(['checkbox', 'radio', 'button', 'submit', 'reset', 'range', 'color', 'file', 'image'])
const ACTIVE = '[aria-current]:not([aria-current="false"]), [aria-selected="true"], [data-zone-active]'
const OBSERVED = ['disabled', 'hidden', 'inert', 'aria-current', 'aria-selected', 'aria-disabled', 'data-zone-active', 'data-zone-skip']

/** Text fields keep their own Tab stop: arrows there edit text. */
export function isTextField(el: Element): boolean {
  if (el instanceof HTMLTextAreaElement) return true
  if (el instanceof HTMLInputElement) return !TEXT_INPUT_TYPES.has(el.type)
  return el instanceof HTMLElement && el.isContentEditable
}

function isRendered(el: Element): boolean {
  // ``checkVisibility`` covers display:none and closed <details>; engines
  // without it (older WebKit, test DOMs) count every element as rendered.
  const check = (el as HTMLElement & { checkVisibility?: () => boolean }).checkVisibility
  return typeof check === 'function' ? check.call(el) : true
}

function isItem(zone: Element, el: Element): boolean {
  if (el.closest(`[${ZONE}]`) !== zone) return false
  if (isTextField(el)) return false
  if (el.closest('[data-zone-skip], [inert], [hidden]')) return false
  if ((el as HTMLButtonElement).disabled) return false
  if (el.getAttribute('role') === 'tab') return el.getAttribute('aria-selected') === 'true'
  // Natively out of Tab order (a scroll container, a menu's items): leave it.
  if (!el.hasAttribute(ITEM) && el.getAttribute('tabindex') === '-1') return false
  return true
}

/** The zone's items in DOM order. */
export function zoneItems(zone: Element): HTMLElement[] {
  return Array.from(zone.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((el) => isItem(zone, el))
}

/** Skipped controls of this zone (not of a nested one). */
function skippedControls(zone: Element): HTMLElement[] {
  return Array.from(zone.querySelectorAll<HTMLElement>('[data-zone-skip]'))
    .flatMap((root) => [root, ...Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE))])
    .filter((el) => el.matches(FOCUSABLE) && el.closest(`[${ZONE}]`) === zone && !isTextField(el))
}

interface ZoneState {
  current: HTMLElement | null
}

const states = new WeakMap<Element, ZoneState>()

function pick(zone: Element, items: HTMLElement[], entry: ZoneEntry): HTMLElement | null {
  const rendered = items.filter(isRendered)
  const state = states.get(zone)
  // Focus that reached an item before it qualified (a tab selected after it
  // was focused) still counts as the keyboard position.
  const focused = document.activeElement
  if (state && focused instanceof HTMLElement && rendered.includes(focused)) state.current = focused
  const current = state?.current
  if (current && rendered.includes(current)) return current
  if (entry === 'active') {
    // The active control itself, else the first item inside an active row.
    const active = rendered.find((el) => el.matches(ACTIVE))
      ?? rendered.find((el) => {
        const host = el.closest(ACTIVE)
        return host !== null && zone.contains(host)
      })
    if (active) return active
  }
  if (entry === 'last') return rendered.at(-1) ?? null
  return rendered[0] ?? null
}

/** Give the zone's current item ``tabIndex`` 0 and every other item -1. */
export function rove(zone: Element, entry: ZoneEntry = 'first'): HTMLElement | null {
  const items = zoneItems(zone)
  const current = pick(zone, items, entry)
  for (const item of items) {
    item.setAttribute(ITEM, '')
    const index = item === current ? '0' : '-1'
    if (item.getAttribute('tabindex') !== index) item.setAttribute('tabindex', index)
  }
  for (const skipped of skippedControls(zone)) {
    if (skipped.getAttribute('tabindex') !== '-1') skipped.setAttribute('tabindex', '-1')
  }
  return current
}

function setCurrent(zone: Element, item: HTMLElement): void {
  const state = states.get(zone)
  if (state) state.current = item
  for (const other of zoneItems(zone)) {
    other.setAttribute(ITEM, '')
    const index = other === item ? '0' : '-1'
    if (other.getAttribute('tabindex') !== index) other.setAttribute('tabindex', index)
  }
}

/** Focus the zone's current item (or its entry item); false when it has none. */
export function focusZone(zone: Element, entry: ZoneEntry = 'first'): boolean {
  const item = rove(zone, entry)
  if (!item) return false
  item.focus()
  return document.activeElement === item
}

function itemOf(zone: Element, target: EventTarget | null): HTMLElement | null {
  if (!(target instanceof Element)) return null
  const el = target.closest<HTMLElement>(FOCUSABLE)
  return el && zone.contains(el) && isItem(zone, el) ? el : null
}

function attachZone(zone: HTMLElement, latest: { readonly current: FocusZoneOptions }): () => void {
  states.set(zone, { current: null })
  zone.setAttribute(ZONE, latest.current.orientation)
  const { role, label, orientation } = latest.current
  if (role) {
    zone.setAttribute('role', role)
    zone.setAttribute('aria-orientation', orientation)
  }
  if (label) zone.setAttribute('aria-label', label)

  const entry = () => latest.current.entry ?? 'first'
  let frame: number | null = null
  const schedule = () => {
    if (frame !== null) return
    frame = requestAnimationFrame(() => {
      frame = null
      rove(zone, entry())
    })
  }
  rove(zone, entry())

  const observer = new MutationObserver(schedule)
  observer.observe(zone, { childList: true, subtree: true, attributes: true, attributeFilter: OBSERVED })

  const onFocusIn = (event: FocusEvent) => {
    const item = itemOf(zone, event.target)
    if (item) setCurrent(zone, item)
  }

  // A click sets the keyboard position, so arrows continue from the row
  // that was clicked. WebKit does not focus buttons on click by itself.
  const onPointerDown = (event: PointerEvent) => {
    if (event.button !== 0) return
    const item = itemOf(zone, event.target)
    if (!item) return
    setCurrent(zone, item)
    if (document.activeElement !== item) item.focus({ preventScroll: true })
  }

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return
    const item = itemOf(zone, event.target)
    // An open menu or listbox owns the arrow keys of its trigger.
    if (!item || (item.hasAttribute('aria-haspopup') && item.getAttribute('aria-expanded') === 'true')) return
    const { orientation: axis, wrap = false } = latest.current
    const back = axis === 'vertical' ? 'ArrowUp' : 'ArrowLeft'
    const forward = axis === 'vertical' ? 'ArrowDown' : 'ArrowRight'
    if (![back, forward, 'Home', 'End'].includes(event.key)) return
    const items = zoneItems(zone).filter(isRendered)
    const index = items.indexOf(item)
    if (index < 0) return
    let next = index
    if (event.key === 'Home') next = 0
    else if (event.key === 'End') next = items.length - 1
    else {
      next = index + (event.key === forward ? 1 : -1)
      if (wrap) next = (next + items.length) % items.length
      else next = Math.max(0, Math.min(items.length - 1, next))
    }
    // Handled even at an edge: WebKit beeps on unhandled arrow keys.
    event.preventDefault()
    const target = items[next]
    if (!target || target === item) return
    setCurrent(zone, target)
    target.focus({ preventScroll: true })
    target.scrollIntoView?.({ block: 'nearest', inline: 'nearest' })
  }

  zone.addEventListener('focusin', onFocusIn)
  zone.addEventListener('pointerdown', onPointerDown)
  zone.addEventListener('keydown', onKeyDown)
  return () => {
    observer.disconnect()
    if (frame !== null) cancelAnimationFrame(frame)
    zone.removeEventListener('focusin', onFocusIn)
    zone.removeEventListener('pointerdown', onPointerDown)
    zone.removeEventListener('keydown', onKeyDown)
    zone.removeAttribute(ZONE)
    states.delete(zone)
  }
}

export function useFocusZone<T extends HTMLElement>(ref: RefObject<T | null>, options: FocusZoneOptions): void {
  const latest = useRef(options)
  latest.current = options
  const enabled = options.enabled ?? true
  const attached = useRef<{ el: HTMLElement; detach: () => void } | null>(null)

  // Checked after every render: the zone element can be replaced (a parent
  // switching between a plain and an animated wrapper) without the ref
  // object changing.
  useEffect(() => {
    const el = enabled ? ref.current : null
    if (attached.current?.el === el) return
    attached.current?.detach()
    attached.current = el ? { el, detach: attachZone(el, latest) } : null
  })
  useEffect(() => () => {
    attached.current?.detach()
    attached.current = null
  }, [])
}
