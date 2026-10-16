import { renderHook, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { useExecutionEvents } from './use-execution-events'

afterEach(() => vi.unstubAllGlobals())

describe('useExecutionEvents', () => {
  it('calibrates execution status before draining terminal events by cursor', async () => {
    const requests: string[] = []
    let executionReads = 0
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const path = String(input)
      requests.push(path)
      if (path.endsWith('/executions/execution-1')) {
        executionReads += 1
        return json({ id: 'execution-1', status: executionReads === 1 ? 'running' : 'succeeded' })
      }
      if (path.includes('/executions/execution-1/events?after=0&limit=200')) {
        return json({ items: events(1, 4, 'queued'), nextCursor: 4 })
      }
      if (path.includes('/executions/execution-1/events?after=4&limit=200')) {
        return json({ items: [...events(5, 8, 'succeeded'), event(9, 'execution.succeeded', 'succeeded')], nextCursor: 9 })
      }
      throw new Error(`Unexpected request ${path}`)
    }))

    const { result } = renderHook(() => useExecutionEvents('execution-1'))
    await waitFor(() => expect(result.current.events).toHaveLength(4))
    await waitFor(() => expect(result.current.events).toHaveLength(9), { timeout: 2_000 })

    expect(result.current.events.at(-1)).toMatchObject({ sequence: 9, eventType: 'execution.succeeded', status: 'succeeded' })
    expect(result.current.running).toBe(false)
    expect(requests.slice(0, 4)).toEqual([
      '/api/v1/executions/execution-1',
      '/api/v1/executions/execution-1/events?after=0&limit=200',
      '/api/v1/executions/execution-1',
      '/api/v1/executions/execution-1/events?after=4&limit=200',
    ])
  })
  it('keeps failed execution events visible when the detail refresh fails', async () => {
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => String(input).includes('/events?')
      ? json({ items: [event(1, 'node.failed', 'failed'), event(2, 'execution.failed', 'failed')], nextCursor: 2 })
      : new Response(JSON.stringify({ code: 'RUNTIME_QUERY_UNAVAILABLE', message: 'Query unavailable' }), { status: 503, headers: { 'Content-Type': 'application/json' } })))
    const { result } = renderHook(() => useExecutionEvents('execution-1'))
    await waitFor(() => expect(result.current.events).toHaveLength(2))
    expect(result.current.running).toBe(false)
    expect(result.current.error).toBeTruthy()
  })

  it('records a terminal status even when the events endpoint is unavailable', async () => {
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => String(input).includes('/events?')
      ? new Response('{}', { status: 503, headers: { 'Content-Type': 'application/json' } })
      : json({ id: 'execution-1', status: 'failed' })))
    const { result } = renderHook(() => useExecutionEvents('execution-1'))
    await waitFor(() => expect(result.current.error).toBeTruthy())
    expect(result.current.running).toBe(false)
  })

})

function events(from: number, to: number, status: string) {
  return Array.from({ length: to - from + 1 }, (_, index) => event(from + index, `event.${from + index}`, status))
}

function event(sequence: number, eventType: string, status: string) {
  return { sequence, eventType, status, summary: {}, occurredAt: '2026-08-21T04:59:49Z' }
}

function json(value: unknown) {
  return new Response(JSON.stringify(value), { headers: { 'Content-Type': 'application/json' } })
}
