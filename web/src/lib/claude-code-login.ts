/**
 * Sign the server's `claude` CLI in from the app: open a new terminal on the
 * server (the dock's terminal is a shell there) and run `claude auth login`.
 * The CLI opens the browser on the server and stores its own login; the app
 * never sees credentials. The Claude desktop app and IDE extensions keep
 * their own logins, so a server-run Claude Code turn can be logged out while
 * those work.
 */
import { APP_EVENTS, dispatchAppEvent } from '@/lib/app-events'
import { useTerminalStore } from '@/stores/useTerminalStore'

const LOGIN_MARKERS = ['claude auth login', 'not logged in', 'oauth', 'failed to authenticate', '/login']

/** A Claude Code turn failed because the server's CLI is not signed in. */
export function isClaudeCodeLoginError(message: string, sessionModel: string | null | undefined): boolean {
  if (!sessionModel?.startsWith('claude-code:')) return false
  const lower = message.toLowerCase()
  return LOGIN_MARKERS.some((m) => lower.includes(m))
}

export function startClaudeCodeLogin(workspace: string): string {
  const store = useTerminalStore.getState()
  const id = store.open({ workspace }, workspace)
  store.rename(id, 'Claude login')
  store.runWhenConnected(id, 'claude auth login\r')
  // Shows the dock and focuses the newest terminal (this one).
  dispatchAppEvent(APP_EVENTS.openTerminal)
  return id
}
