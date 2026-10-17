import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../../app/i18n'
import { ToastProvider } from '../../shared/ui/toast'
import { AgentSessionsPage } from './agent-sessions-page'

describe('Agent session diagnostics', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('zh-CN')
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), 'http://agentx.test')
      if (url.pathname === '/api/v1/agent-sessions' && !init?.method) {
        return jsonResponse({ items: [{ sessionKey: 'session-1', sessionId: 'session-id', stableAgentNodeKey: 'agent-1', applicationId: 'app-1', stateVersion: 4, fencingToken: 2, openOperationId: null, terminalState: 'completed', bundleHash: 'sha256:bundle', modelVersion: 'model:v2', updatedAt: '2026-08-25T00:00:00Z' }] })
      }
      if (url.pathname === '/api/v1/agent-sessions/session-1/agent-1') {
        return jsonResponse({ summary: { sessionKey: 'session-1', sessionId: 'session-id', stableAgentNodeKey: 'agent-1', applicationId: 'app-1', stateVersion: 4, fencingToken: 2, bundleHash: 'sha256:bundle', modelVersion: 'model:v2', updatedAt: '2026-08-25T00:00:00Z' }, entries: [{ entryId: 'entry-1', entryKind: 'message.user', operationId: 'op-1', createdAt: '2026-08-25T00:00:00Z' }], usages: [], compaction: null, recovery: null })
      }
      if (url.pathname === '/api/v1/agent-sessions/clear' || url.pathname === '/api/v1/agent-subject-memory/clear') return jsonResponse({ status: 'cleared' })
      if (url.pathname === '/api/v1/agent-subject-memory/audit') return jsonResponse({ items: [], next: null })
      return new Response(JSON.stringify({ code: 'NOT_FOUND', message: 'not found' }), { status: 404, headers: { 'Content-Type': 'application/json' } })
    }))
  })

  it('loads a session, opens diagnostics, and clears it', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(<QueryClientProvider client={queryClient}><ToastProvider><MemoryRouter><AgentSessionsPage /></MemoryRouter></ToastProvider></QueryClientProvider>)

    const row = await screen.findByText('session-1')
    expect(row).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: '查看会话' }))
    expect(await screen.findByText('message.user')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: '清除会话' }))
    const dialog = await screen.findByRole('dialog')
    fireEvent.click(within(dialog).getByRole('button', { name: '清除会话' }))
    await waitFor(() => expect(screen.queryByText('会话诊断')).not.toBeInTheDocument())
    expect(vi.mocked(fetch).mock.calls.some(([, init]) => init?.method === 'POST' && String(init.body).includes('session-1'))).toBe(true)
  })

  it('shows memory audit loading and error states without hiding diagnostics', async () => {
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), 'http://agentx.test')
      if (url.pathname === '/api/v1/agent-sessions' && !init?.method) return jsonResponse({ items: [{ sessionKey: 'session-1', sessionId: 'session-id', stableAgentNodeKey: 'agent-1', applicationId: 'app-1', stateVersion: 1, fencingToken: 1, bundleHash: 'bundle', modelVersion: 'model', updatedAt: '2026-08-25T00:00:00Z' }] })
      if (url.pathname === '/api/v1/agent-sessions/session-1/agent-1') return jsonResponse({ summary: { sessionKey: 'session-1', sessionId: 'session-id', stableAgentNodeKey: 'agent-1', applicationId: 'app-1', stateVersion: 1, fencingToken: 1, bundleHash: 'bundle', modelVersion: 'model', updatedAt: '2026-08-25T00:00:00Z' }, memoryTargets: [{ memoryResourceId: 'memory-1', scope: { tenantId: 'tenant-1', applicationId: 'app-1', workflowId: 'workflow-1', environmentId: 'env-1', stableNodeId: 'node-1', user: { source: 'api_parameter', sourceId: 'api', externalUserId: 'customer-42' } } }], entries: [], usages: [] })
      if (url.pathname === '/api/v1/agent-subject-memory/audit') return new Response(JSON.stringify({ code: 'FORBIDDEN', message: '无权访问主体记忆' }), { status: 403, headers: { 'Content-Type': 'application/json' } })
      return jsonResponse({ status: 'cleared' })
    }))
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(<QueryClientProvider client={queryClient}><ToastProvider><MemoryRouter><AgentSessionsPage /></MemoryRouter></ToastProvider></QueryClientProvider>)
    await screen.findByText('session-1')
    fireEvent.click(screen.getByRole('button', { name: '查看会话' }))
    await screen.findByText('用户记忆审计')
    expect(await screen.findByText('customer-42')).toBeInTheDocument()
    expect(screen.queryByPlaceholderText('UUID')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: '加载审计' }))
    await waitFor(() => expect(screen.getByText('无权访问主体记忆')).toBeInTheDocument())
    expect(screen.getByText('会话诊断')).toBeInTheDocument()
  })
})

function jsonResponse(value: unknown) {
  return new Response(JSON.stringify(value), { headers: { 'Content-Type': 'application/json' } })
}
