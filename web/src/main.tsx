import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { AppErrorBoundary } from './components/AppErrorBoundary'
import { installDesktopAuth } from './api/auth'
import { initBackgroundThrottle } from './lib/background-throttle'
import { installDesktopShell } from './lib/desktop-shell'
import { initTheme } from './lib/theme'

// Install the desktop session token interceptor *before* any other module
// has a chance to capture a reference to the original `window.fetch`.
installDesktopAuth()
initBackgroundThrottle()
initTheme()
installDesktopShell()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <AppErrorBoundary>
      <App />
    </AppErrorBoundary>
  </StrictMode>,
)
