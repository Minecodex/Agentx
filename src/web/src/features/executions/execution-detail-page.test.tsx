import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../../app/i18n'
import { useAuth } from '../../app/providers/auth-provider'
import { ToastProvider } from '../../shared/ui/toast'
import { ExecutionDetailPage } from './execution-detail-page'

vi.mock('../../app/providers/auth-provider', () => ({ useAuth: vi.fn() }))

const execution = {
  id: 'execution-1', workflowId: 'workflow-1', workflowName: 'Order recovery', workflowVersionId: 'version-1', workflowVersionNumber: 4,
  invocationId: null, sessionId: null, traceId: 'trace-1', triggerType: 'manual', executionType: 'whole', parentExecutionId: null,
  callerExecutionId: null, forkCheckpointId: null, status: 'waiting', startedAt: '2026-08-03T10:00:00Z', endedAt: null,
  durationMs: 1200, costMicros: 0, errorCode: null, errorMessage: null,
  inputTokens: 0, outputTokens: 0, input: { amount: 42 }, output: { result: 'waiting' },
}

const nodes = { items: [
  {
    id: 'node-execution-1', executionId: 'execution-1', nodeId: 'source', nodeName: 'Source', nodeType: 'set', nodeVersion: 1,
    generation: 0, activationSlot: 0, runIndex: 0, iterationIndex: 0, status: 'succeeded', capability: 'builtin', sideEffectLevel: 'none',
    input: { main: [{ json: { amount: 42 } }] }, output: { main: [{ json: { amount: 42 } }] }, errorCode: null, errorMessage: null,
    startedAt: '2026-08-03T10:00:00Z', endedAt: '2026-08-03T10:00:00Z', attempts: [{ id: 'attempt-1', attemptNumber: 1, status: 'succeeded', workerInstanceId: 'worker-a', deadlineAt: null, errorCode: null, errorMessage: null, startedAt: '2026-08-03T10:00:00Z', endedAt: '2026-08-03T10:00:00Z' }], lineage: [],
  },
  {
    id: 'node-execution-2', executionId: 'execution-1', nodeId: 'code-charge', nodeName: 'Code charge', nodeType: 'code', nodeVersion: 1,
    generation: 0, activationSlot: 0, runIndex: 0, iterationIndex: 0, status: 'waiting', capability: 'sandbox', sideEffectLevel: 'irreversible',
    input: { main: [{ json: { amount: 42 } }] }, output: null, errorCode: null, errorMessage: null, startedAt: '2026-08-03T10:00:01Z', endedAt: null,
    attempts: [], lineage: [{ deliveryId: 'delivery-1', targetItemIndex: 0, sourceNodeExecutionId: 'node-execution-1', sourceRunIndex: 0, sourceOutputIndex: 0, sourceItemIndex: 0 }],
  },
] }

function response(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } })
}

function installFetch(options: { execution?: unknown; nodes?: unknown; traceStatus?: number; onRequest?: (path: string) => void } = {}) {
  vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
    const path = new URL(String(input), 'http://agentx.test').pathname
    options.onRequest?.(path)
    if (path === '/api/v1/executions/execution-1') return response(options.execution ?? execution)
    if (path.endsWith('/nodes')) return response(options.nodes ?? nodes)
    if (path.includes('/nodes/')) return response(nodes.items.find((node) => path.endsWith(node.id)))
    if (path.endsWith('/checkpoints')) return response({ items: [{ id: 'checkpoint-1', executionId: 'execution-1', nodeExecutionId: 'node-execution-1', sequenceNumber: 2, checkpointType: 'node_completed', stateHash: 'sha256-state', activationCount: 2, deliveryCount: 1, createdAt: '2026-08-03T10:00:01Z' }] })
    if (path.endsWith('/events')) return response({ items: [{ sequence: 3, eventType: 'execution.waiting', status: 'waiting', summary: { reason: 'approval' }, occurredAt: '2026-08-03T10:00:01Z' }] })
    if (path.endsWith('/trace')) return response({ executionId: 'execution-1', traceId: 'trace-1', nextCursor: null, complete: options.traceStatus !== 202, degraded: false, warningCode: options.traceStatus === 202 ? 'TRACE_DELAYED' : null, totalSpans: 2, expectedWatermark: 2, ingestedWatermark: options.traceStatus === 202 ? 1 : 2, spans: [
      { spanId: 'span-root', parentSpanId: null, spanKind: 'execution', spanName: 'Order recovery', status: 'running', startedAt: '2026-08-03T10:00:00Z', endedAt: null, durationMs: null, costMicros: 0, hasDetails: false },
      { spanId: 'span-node', parentSpanId: 'span-root', spanKind: 'node', spanName: 'Code charge', status: 'waiting', startedAt: '2026-08-03T10:00:01Z', endedAt: null, durationMs: null, nodeExecutionId: 'node-execution-2', costMicros: 0, hasDetails: true },
    ] })
    if (path.endsWith('/side-effect-confirmations')) return response({ accepted: true, replayed: false })
    if (path === '/api/v1/approvals') return response({ items: [{ id: 'approval-1', executionId: 'execution-1', workflowId: 'workflow-1', workflowName: 'Order recovery', nodeId: 'approval-review', title: 'Approve charge', description: null, buttons: [{ id: 'approved', label: 'Approve' }], decision: null, status: 'pending', claimedBy: null, claimedByName: null, resumeStatus: 'pending', requestPayload: {}, deadlineAt: null, version: 1, createdAt: '2026-08-03T10:00:01Z' }], page: 1, pageSize: 100, total: 1 })
    return response({ code: 'NOT_FOUND', message: path, requestId: 'test' }, 404)
  }))
}

function renderPage(permissions: string[]) {
  vi.mocked(useAuth).mockReturnValue({
    sessionId: 1,
    status: 'authenticated', user: undefined, changeToken: undefined,
    setup: vi.fn(), login: vi.fn(), changePassword: vi.fn(), logout: vi.fn(),
    hasPermission: (permission) => permissions.includes(permission),
  })
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(<QueryClientProvider client={queryClient}><ToastProvider><MemoryRouter initialEntries={['/executions/execution-1']}><Routes><Route element={<ExecutionDetailPage />} path="/executions/:id" /></Routes></MemoryRouter></ToastProvider></QueryClientProvider>)
}

describe('execution recovery workbench', () => {
  beforeEach(async () => {
    installFetch()
    await i18n.changeLanguage('zh-CN')
  })

  it('keeps recovery focused on approvals, checkpoints, events and fork risk preview', async () => {
    const requests: string[] = []
    installFetch({ onRequest: (path) => requests.push(path) })
    renderPage(['execution:view', 'execution:fork', 'execution:cancel', 'approval:view'])

    const heading = await screen.findByRole('heading', { name: 'Order recovery' })
    expect(heading).toHaveClass('min-w-0', 'flex-1', 'truncate')
    expect(heading.parentElement?.parentElement?.parentElement).toHaveClass('grid-cols-[2.25rem_minmax(0,1fr)]', 'md:flex')
    fireEvent.click(screen.getByRole('tab', { name: '恢复' }))
    expect((await screen.findAllByText('Code charge')).length).toBeGreaterThan(0)
    expect(await screen.findByText('Approve charge')).toBeInTheDocument()
    expect(screen.getByText('#2 node_completed')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Source/ })).not.toBeInTheDocument()
    expect(screen.getByText('execution.waiting')).toBeInTheDocument()

    const forkButton = screen.getByRole('button', { name: '派生执行' })
    fireEvent.click(forkButton)
    expect(screen.getByRole('dialog')).toBeVisible()
    expect(screen.getByText('执行预览')).toBeInTheDocument()
    expect(screen.getByText(/不可逆节点需要明确决策/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: '取消' }))
    await waitFor(() => expect(forkButton).toHaveFocus())

    fireEvent.click(screen.getByRole('button', { name: '处理' }))
    expect(screen.getByRole('dialog', { name: '副作用确认' })).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: '确认决策' }))
    await waitFor(() => expect(requests).toContain('/api/v1/executions/execution-1/side-effect-confirmations'))
  }, 10_000)

  it('does not render recovery commands without their permissions', async () => {
    renderPage([])
    expect(await screen.findByText('Order recovery')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: '派生执行' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument()
    expect(screen.queryByText('Approve charge')).not.toBeInTheDocument()
  })

  it('refreshes runtime snapshots once when the execution reaches a terminal state', async () => {
    let nodeRequests = 0
    installFetch({
      execution: { ...execution, status: 'succeeded', endedAt: '2026-08-03T10:00:03Z' },
      onRequest: (path) => { if (path.endsWith('/nodes')) nodeRequests += 1 },
    })
    renderPage([])

    expect(await screen.findByText('Order recovery')).toBeInTheDocument()
    await waitFor(() => expect(nodeRequests).toBeGreaterThanOrEqual(2))
  })

  it('keeps the authoritative terminal state visible while trace ingestion is delayed', async () => {
    installFetch({
      execution: { ...execution, status: 'succeeded', endedAt: '2026-08-03T10:00:03Z' },
      traceStatus: 202,
    })
    renderPage([])

    expect(await screen.findByText('Order recovery')).toBeInTheDocument()
    expect((await screen.findAllByText('Trace 正在同步…')).length).toBeGreaterThan(0)
    expect(screen.getAllByText('成功').length).toBeGreaterThan(0)
  })
})
