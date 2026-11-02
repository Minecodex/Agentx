import type { ReactNode } from 'react'
import { Navigate, useLocation } from 'react-router-dom'

import { useAuth } from './providers/auth-provider'

export function Protected({ children }: { children: ReactNode }) {
  const { status } = useAuth(); const location = useLocation()
  const returnState={from:window.location.pathname===location.pathname?`${location.pathname}${window.location.search}${window.location.hash}`:`${location.pathname}${location.search}${location.hash}`}
  if (status === 'loading') return <div className="grid min-h-screen place-items-center bg-background text-sm text-muted-foreground">Agentx…</div>
  if (status === 'setup') return <Navigate replace to="/setup" />
  if (status === 'password-change') return <Navigate replace state={returnState} to="/change-password" />
  if (status !== 'authenticated') return <Navigate replace state={returnState} to="/login" />
  return children
}

export function PublicRoute({ kind, children }: { kind: 'setup' | 'login' | 'change-password'; children: ReactNode }) {
  const { status } = useAuth()
  const location=useLocation()
  const requested=(location.state as {from?:unknown}|null)?.from
  const returnTo=typeof requested==='string'&&requested.startsWith('/')&&!requested.startsWith('//')&&!requested.includes('\\')&&!Array.from(requested).some(character=>character.charCodeAt(0)<32)?requested:'/'
  if (status === 'loading') return <div className="grid min-h-screen place-items-center bg-background text-sm text-muted-foreground">Agentx…</div>
  if (status === 'authenticated') return <Navigate replace to={returnTo} />
  if (status === 'setup' && kind !== 'setup') return <Navigate replace to="/setup" />
  if (status !== 'setup' && kind === 'setup') return <Navigate replace to="/login" />
  if (status === 'password-change' && kind !== 'change-password') return <Navigate replace state={location.state} to="/change-password" />
  if (status !== 'password-change' && kind === 'change-password') return <Navigate replace to="/login" />
  return children
}

export function RequirePermission({ permission, children }: { permission: string; children: ReactNode }) {
  const { status, hasPermission } = useAuth()
  if (status !== 'authenticated') return children
  return hasPermission(permission) ? children : <Navigate replace to="/403" />
}

export function RequireAnyPermission({ permissions, children }: { permissions: string[]; children: ReactNode }) {
  const { status, hasPermission } = useAuth()
  if (status !== 'authenticated') return children
  return permissions.some(hasPermission) ? children : <Navigate replace to="/403" />
}
