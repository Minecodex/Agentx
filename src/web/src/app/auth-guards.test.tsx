import { render, screen } from '@testing-library/react'
import type { ReactNode } from 'react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { Protected, PublicRoute, RequirePermission } from './auth-guards'
import { useAuth } from './providers/auth-provider'

vi.mock('./providers/auth-provider', () => ({ useAuth: vi.fn() }))

const auth = {
  sessionId: 1,
  status: 'authenticated' as const,
  user: undefined,
  changeToken: undefined,
  setup: vi.fn(),
  login: vi.fn(),
  changePassword: vi.fn(),
  logout: vi.fn(),
  hasPermission: vi.fn(() => true),
}

function renderRoutes(element: ReactNode, path = '/') {
  render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route element={element} path="*" />
        <Route element={<span>setup-page</span>} path="/setup" />
        <Route element={<span>login-page</span>} path="/login" />
        <Route element={<span>change-page</span>} path="/change-password" />
        <Route element={<span>forbidden-page</span>} path="/403" />
      </Routes>
    </MemoryRouter>,
  )
}

describe('authentication route guards', () => {
  beforeEach(() => {
    vi.mocked(useAuth).mockReturnValue({ ...auth, hasPermission: vi.fn(() => true) })
  })

  it.each([
    ['setup', 'setup-page'],
    ['anonymous', 'login-page'],
    ['password-change', 'change-page'],
  ] as const)('redirects protected routes for %s state', (status, expected) => {
    vi.mocked(useAuth).mockReturnValue({ ...auth, status })
    renderRoutes(<Protected><span>protected-page</span></Protected>, '/organization')
    expect(screen.getByText(expected)).toBeInTheDocument()
  })

  it('allows an authenticated user through protected routes', () => {
    renderRoutes(<Protected><span>protected-page</span></Protected>)
    expect(screen.getByText('protected-page')).toBeInTheDocument()
  })

  it('keeps a temporary-password user on the change-password route', () => {
    vi.mocked(useAuth).mockReturnValue({ ...auth, status: 'password-change' })
    renderRoutes(<PublicRoute kind="change-password"><span>change-form</span></PublicRoute>, '/temporary-password-entry')
    expect(screen.getByText('change-form')).toBeInTheDocument()
  })

  it('redirects missing permissions to the unified forbidden page', () => {
    vi.mocked(useAuth).mockReturnValue({ ...auth, hasPermission: vi.fn(() => false) })
    renderRoutes(<RequirePermission permission="user:view"><span>organization-page</span></RequirePermission>)
    expect(screen.getByText('forbidden-page')).toBeInTheDocument()
  })
})
