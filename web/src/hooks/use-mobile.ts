import * as React from "react"

const MOBILE_BREAKPOINT = 768
/** Shared media query used by both `useIsMobile` and `useMobileViewportGuards` to ensure consistent breakpoint detection. */
export const MOBILE_QUERY = `(max-width: ${MOBILE_BREAKPOINT - 1}px), (max-height: 580px)`

// One MediaQueryList for every caller: every Tooltip uses this hook, so a long
// transcript mounts hundreds of them. Keyed by the `matchMedia` function so a
// test that swaps it gets a fresh list.
let shared: { matchMedia: typeof window.matchMedia; mql: MediaQueryList } | null = null

function mobileQueryList(): MediaQueryList {
  if (!shared || shared.matchMedia !== window.matchMedia) {
    shared = { matchMedia: window.matchMedia, mql: window.matchMedia(MOBILE_QUERY) }
  }
  return shared.mql
}

function subscribe(onChange: () => void): () => void {
  const mql = mobileQueryList()
  mql.addEventListener("change", onChange)
  return () => mql.removeEventListener("change", onChange)
}

function getSnapshot(): boolean {
  return typeof window !== "undefined" && mobileQueryList().matches
}

/** Read synchronously, so the first render already has the right layout. */
export function useIsMobile() {
  return React.useSyncExternalStore(subscribe, getSnapshot, () => false)
}
