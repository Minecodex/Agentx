import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import * as Tooltip from '@radix-ui/react-tooltip'
import { RouterProvider } from 'react-router-dom'

import './app/i18n'
import { registerChunkRecovery } from './app/chunk-recovery'
import { router } from './app/router'
import { ThemeProvider } from './app/providers/theme-provider'
import { AuthProvider } from './app/providers/auth-provider'
import { ToastProvider } from './shared/ui/toast'
import './styles/globals.css'

registerChunkRecovery(import.meta.url)

const queryClient = new QueryClient()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <ThemeProvider>
        <AuthProvider>
          <Tooltip.Provider delayDuration={300}>
            <ToastProvider>
              <RouterProvider router={router} />
            </ToastProvider>
          </Tooltip.Provider>
        </AuthProvider>
      </ThemeProvider>
    </QueryClientProvider>
  </StrictMode>,
)
