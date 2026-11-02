/* oxlint-disable react/only-export-components */
import { useQueryClient } from '@tanstack/react-query'
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest, currentAuthSession, invalidateAuthSession, jsonBody, publicRequest, refreshAccessToken, setAccessToken, setRefreshHandler } from '../../shared/api/client'
import type { AuthResponse, AuthUser } from '../../shared/api/types'

type AuthStatus = 'loading' | 'setup' | 'anonymous' | 'password-change' | 'authenticated'
type SetupInput = { companyName: string; adminUsername: string; adminDisplayName: string; password: string; locale: string; timezone: string }
type AuthContextValue = {
  status: AuthStatus; user?: AuthUser; changeToken?: string; sessionId:number;
  setup: (input: SetupInput) => Promise<void>; login: (username: string, password: string) => Promise<void>;
  changePassword: (password: string) => Promise<void>; logout: () => Promise<void>; hasPermission: (key: string) => boolean;
}
const AuthContext = createContext<AuthContextValue | null>(null)

export function AuthProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<AuthStatus>('loading')
  const [user, setUser] = useState<AuthUser>()
  const [changeToken, setChangeToken] = useState<string>()
  const [sessionId,setSessionId]=useState(currentAuthSession)
  const queryClient = useQueryClient()
  const { i18n } = useTranslation()
  const refreshInFlight = useRef<Promise<boolean>>()
  const authVersion = useRef(0)
  const principal=useRef('')

  const loadMe = useCallback(async () => {
    const me = await apiRequest<AuthUser>('/auth/me')
    const identity=`${me.companyId}:${me.id}`
    if(principal.current&&principal.current!==identity)setSessionId(invalidateAuthSession())
    principal.current=identity
    setUser(me); setStatus('authenticated')
    if (!window.localStorage.getItem('agentx.locale') && (me.locale === 'zh-CN' || me.locale === 'en-US')) void i18n.changeLanguage(me.locale)
  }, [i18n])

  const refresh = useCallback(() => {
    const version = authVersion.current
    refreshInFlight.current ??= (async () => {
      try {
        const response = await refreshAccessToken()
        if (authVersion.current !== version) return true
        setAccessToken(response.accessToken); await loadMe(); return true
      } catch {
        if (authVersion.current !== version) return true
        setSessionId(invalidateAuthSession()); setAccessToken(); setUser(undefined); setStatus('anonymous'); queryClient.clear(); return false
      }
    })().finally(() => { refreshInFlight.current = undefined })
    return refreshInFlight.current
  }, [loadMe, queryClient])

  useEffect(() => { setRefreshHandler(refresh) }, [refresh])
  useEffect(() => {
    let active = true
    void publicRequest<{ required: boolean }>('/bootstrap/status').then(async ({ required }) => {
      if (!active) return
      if (required) { setStatus('setup'); return }
      await refresh()
    }).catch(() => { if (active) setStatus('anonymous') })
    return () => { active = false }
  }, [refresh])

  const acceptAuth = async (response: AuthResponse) => {
    authVersion.current += 1
    setSessionId(invalidateAuthSession())
    if (response.passwordChangeRequired) { setChangeToken(response.changePasswordToken ?? undefined); setStatus('password-change'); return }
    setAccessToken(response.accessToken); await loadMe()
  }
  const setup = async (input: SetupInput) => acceptAuth(await publicRequest<AuthResponse>('/bootstrap', { method: 'POST', body: jsonBody(input) }))
  const login = async (username: string, password: string) => acceptAuth(await publicRequest<AuthResponse>('/auth/login', { method: 'POST', body: jsonBody({ username, password }) }))
  const changePassword = async (password: string) => {
    if (!changeToken) throw new Error('Change password token is missing')
    await acceptAuth(await publicRequest<AuthResponse>('/auth/change-password', { method: 'POST', body: jsonBody({ token: changeToken, password }) })); setChangeToken(undefined)
  }
  const logout = async () => { authVersion.current += 1; setSessionId(invalidateAuthSession()); try { await apiRequest('/auth/logout', { method: 'POST' }) } finally { setAccessToken(); setUser(undefined); setStatus('anonymous'); queryClient.clear() } }
  const value: AuthContextValue = { status, user, changeToken, sessionId, setup, login, changePassword, logout, hasPermission: (key) => user?.permissions.includes(key) ?? false }
  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>
}

export function useAuth() { const value = useContext(AuthContext); if (!value) throw new Error('useAuth must be used inside AuthProvider'); return value }
