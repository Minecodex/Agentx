import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../../app/i18n'
import type { Execution, NodeExecution } from '../../shared/api/types'
import { TraceWorkspace } from './trace-workspace'

const execution: Execution = {
  source: {kind:'version',versionId:'version-1',versionNumber:1},
  id: 'execution-1', workflowId: 'workflow-1', workflowName: 'Semantic trace', workflowVersionId: 'version-1', workflowVersionNumber: 1,
  invocationId: null, sessionId: null, traceId: 'trace-1', triggerType: 'manual', executionType: 'whole', parentExecutionId: null,
  callerExecutionId: null, forkCheckpointId: null, status: 'succeeded', startedAt: '2026-08-21T04:00:00Z', endedAt: '2026-08-21T04:00:02Z',
  durationMs: 2_000, costMicros: 1_200, costCurrency: 'USD', inputTokens: 8, outputTokens: 12, input: { question: '你是谁？' }, output: { result: '我是 kakj。' },
}

const nodes: NodeExecution[] = [{
  id: 'node-execution-1', executionId: execution.id, nodeId: 'model-1', nodeName: '回答问题', nodeType: 'model', nodeVersion: 1,
  generation: 0, activationSlot: 0, runIndex: 2, iterationIndex: 1, status: 'succeeded', capability: 'model', sideEffectLevel: 'none',
  costMicros: 1_200, costCurrency: 'USD',
  startedAt: '2026-08-21T04:00:00Z', endedAt: '2026-08-21T04:00:02Z', errorCode: null, errorMessage: null, attempts: [], lineage: [],
  input: { main: [{ json: { question: '你是谁？' } }, { json: { question: '请介绍自己。' } }] },
  output: { main: [{ json: { text: '我是 kakj。', usage: { inputTokens: 8, outputTokens: 12, totalTokens: 20 }, finishReason: 'stop' } }], error: [{ json: { code: 'IGNORED_SAMPLE' } }] },
}]

function renderWorkspace() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(<QueryClientProvider client={client}><TraceWorkspace active execution={execution} executionId={execution.id} nodes={nodes} /></QueryClientProvider>)
}

describe('Trace workspace', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('zh-CN')
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url.includes('/trace?')) return new Response(JSON.stringify({ code: 'OBSERVABILITY_UNAVAILABLE', message: 'ClickHouse is paused' }), { status: 503, headers: { 'Content-Type': 'application/json' } })
      return new Response(JSON.stringify({ code: 'NOT_FOUND', message: url }), { status: 404, headers: { 'Content-Type': 'application/json' } })
    }))
  })

  it('renders authoritative Start, node and End data while diagnostics are unavailable', async () => {
    renderWorkspace()

    expect(screen.getByTestId('trace-final-output')).toHaveTextContent('我是 kakj。')
    expect(screen.getByTestId('trace-start-boundary')).toHaveTextContent('开始')
    expect(screen.getByTestId('trace-end-boundary')).toHaveTextContent('我是 kakj。')
    expect(screen.getByTestId('trace-node-card')).toHaveTextContent('回答问题')
    expect(screen.getByTestId('trace-node-card')).toHaveTextContent('run 2 · iteration 1')
    expect(screen.getByTestId('trace-node-card')).toHaveTextContent('2.00 s')
    expect(screen.getByTestId('trace-node-card')).toHaveTextContent('$0.001200')
    expect(screen.getByTestId('trace-node-card').querySelector('button')).toHaveClass('grid-cols-[minmax(160px,1fr)_auto_auto]')
    expect(screen.getByTestId('trace-node-card')).toHaveTextContent('2 个 Item')
    expect(screen.getByTestId('trace-node-card')).toHaveTextContent('我是 kakj。')
    expect(screen.getByTestId('trace-node-card')).toHaveTextContent('8 input · 12 output · 20 total')
    expect(screen.getByTestId('trace-node-view')).toHaveClass('h-full', 'min-h-0', 'overflow-auto')
    expect(screen.getByTestId('trace-node-view').firstElementChild).toHaveClass('before:left-6', 'before:bg-border')
    expect(await screen.findAllByText('Trace 暂不可用；节点的权威输入输出仍可查看。')).not.toHaveLength(0)
  })

  it('uses nodeExecutionId only for an expanded node diagnostic query', async () => {
    renderWorkspace()
    await waitFor(() => expect(vi.mocked(fetch)).toHaveBeenCalled())
    const urls = vi.mocked(fetch).mock.calls.map(([input]) => String(input))
    expect(urls.some((url) => url.includes('nodeExecutionId=node-execution-1'))).toBe(true)

    fireEvent.click(screen.getByRole('tab', { name: '高级瀑布' }))
    await waitFor(() => expect(vi.mocked(fetch).mock.calls.map(([input]) => String(input)).some((url) => url.includes('/trace?limit=200') && !url.includes('nodeExecutionId='))).toBe(true))
  })
})
