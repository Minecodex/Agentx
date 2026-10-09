import { afterEach, describe, expect, it, vi } from 'vitest'

import { ApiClientError, apiRequest, apiRequestCompleted, setAccessToken, setApiErrorTranslator, setRefreshHandler } from './client'

describe('api client', () => {
  afterEach(() => { vi.unstubAllGlobals(); setAccessToken(); setApiErrorTranslator() })

  it('uses the registered locale-aware API error translator', () => {
    setApiErrorTranslator((detail) => `localized:${detail.code}`)
    const error = new ApiClientError(422, { code: 'INVALID_WORKFLOW_DEFINITION', message: 'Workflow definition is invalid', requestId: 'request-1' })

    expect(error.message).toBe('localized:INVALID_WORKFLOW_DEFINITION')
    expect(error.detail.message).toBe('Workflow definition is invalid')
  })

  it('normalizes network failures into a translatable API error', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => { throw new TypeError('Failed to fetch') }))
    setApiErrorTranslator((detail) => `localized:${detail.code}`)

    await expect(apiRequest('/offline')).rejects.toMatchObject({
      message: 'localized:NETWORK_ERROR',
      detail: { code: 'NETWORK_ERROR' },
    })
  })

  it.each([
    { bodyRequestId: 'body-request', expectedRequestId: 'body-request' },
    { bodyRequestId: undefined, expectedRequestId: 'header-request' },
    { bodyRequestId: '  ', expectedRequestId: 'header-request' },
  ])('keeps the request ID available for JSON errors ($expectedRequestId)', async ({ bodyRequestId, expectedRequestId }) => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({
      code: 'UNAUTHORIZED', message: 'authentication failed', requestId: bodyRequestId,
    }), { status: 403, headers: { 'Content-Type': 'application/json', 'X-Request-ID': 'header-request' } })))

    await expect(apiRequest('/forbidden')).rejects.toMatchObject({
      status: 403,
      detail: { code: 'UNAUTHORIZED', requestId: expectedRequestId },
    })
  })

  it('coalesces concurrent unauthorized responses into one refresh', async () => {
    let requests = 0
    vi.stubGlobal('fetch', vi.fn(async () => {
      requests += 1
      if (requests <= 2) return new Response(JSON.stringify({ code: 'EXPIRED', message: 'expired', requestId: 'r' }), { status: 401, headers: { 'Content-Type': 'application/json' } })
      return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { 'Content-Type': 'application/json' } })
    }))
    let refreshes = 0
    setRefreshHandler(async () => { refreshes += 1; await Promise.resolve(); setAccessToken('renewed'); return true })
    const [first, second] = await Promise.all([apiRequest<{ ok: boolean }>('/one'), apiRequest<{ ok: boolean }>('/two')])
    expect(first.ok).toBe(true); expect(second.ok).toBe(true); expect(refreshes).toBe(1)
  })

  it('lets the browser set the multipart boundary for FormData', async () => {
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      expect(new Headers(init?.headers).has('Content-Type')).toBe(false)
      return new Response(JSON.stringify({ id: 'artifact' }), { status: 200, headers: { 'Content-Type': 'application/json' } })
    })
    vi.stubGlobal('fetch', fetchMock)
    const form = new FormData()
    form.append('file', new Blob(['PK\u0003\u0004'], { type: 'application/zip' }), 'skill.zip')

    await apiRequest('/skills/skill-id/artifact', { method: 'POST', body: form })

    expect(fetchMock).toHaveBeenCalledOnce()
  })

  it('accepts successful asynchronous responses without a body', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(null, { status: 202 })))

    await expect(apiRequest('/evaluations/run-id/start', { method: 'POST' })).resolves.toBeUndefined()
  })

  it('still parses successful JSON responses', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ status: 'queued' }), {
      status: 202,
      headers: { 'Content-Type': 'application/json' },
    })))

    await expect(apiRequest<{ status: string }>('/commands')).resolves.toEqual({ status: 'queued' })
  })

  it('treats an accepted-but-incomplete query as a retryable error', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ code: 'TRACE_DELAYED', message: 'trace delayed', requestId: 'trace-1' }), {
      status: 202,
      headers: { 'Content-Type': 'application/json', 'Retry-After': '3' },
    })))

    await expect(apiRequestCompleted('/executions/execution-1/trace')).rejects.toMatchObject({
      status: 202,
      retryAfterSeconds: 3,
      detail: { code: 'TRACE_DELAYED' },
    })
  })
})
