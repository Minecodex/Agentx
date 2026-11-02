import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../../app/i18n'
import { useAuth } from '../../app/providers/auth-provider'
import { ToastProvider } from '../../shared/ui/toast'
import { CredentialDetailPage } from './credential-detail-page'

vi.mock('../../app/providers/auth-provider', () => ({ useAuth: vi.fn() }))

describe('credential details', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('zh-CN')
    vi.mocked(useAuth).mockReturnValue({
      sessionId: 1,
      status: 'authenticated', user: undefined, changeToken: undefined,
      setup: vi.fn(), login: vi.fn(), changePassword: vi.fn(), logout: vi.fn(),
      hasPermission: () => false,
    })
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      if (path.endsWith('/credentials/credential-1')) {
        return new Response(JSON.stringify({ id: 'credential-1', name: 'Model Key', credentialType: 'bearer', storageMode: 'local_encrypted', maskedHint: '••••alue', status: 'active', currentSecretVersion: 2, ownerDepartmentId: 'department-1', version: 2, updatedAt: '2026-08-02T10:00:00Z', secret: 'raw-secret-must-not-render' }), { headers: { 'Content-Type': 'application/json' } })
      }
      if (path.includes('/grants')) return new Response('[]', { headers: { 'Content-Type': 'application/json' } })
      return new Response(JSON.stringify({ items: [], page: 1, pageSize: 100, total: 0 }), { headers: { 'Content-Type': 'application/json' } })
    }))
  })

  it('renders only the masked hint and never an unexpected raw secret field', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(<QueryClientProvider client={queryClient}><ToastProvider><MemoryRouter initialEntries={['/credentials/credential-1']}><Routes><Route element={<CredentialDetailPage />} path="/credentials/:id" /></Routes></MemoryRouter></ToastProvider></QueryClientProvider>)

    expect(await screen.findByText('••••alue')).toBeInTheDocument()
    expect(screen.queryByText('raw-secret-must-not-render')).not.toBeInTheDocument()
  })
})
