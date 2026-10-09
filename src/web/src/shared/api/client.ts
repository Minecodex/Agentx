import type { ApiError, AuthResponse } from './types'

let accessToken: string | undefined
let refreshHandler: (() => Promise<boolean>) | undefined
let refreshPromise: Promise<boolean> | undefined
let apiErrorTranslator: ((detail: ApiError, status: number) => string) | undefined
const runtimeBaseUrl = (window as Window & { __AGENTX_RUNTIME__?: { runtimeBaseUrl?: string } }).__AGENTX_RUNTIME__?.runtimeBaseUrl?.replace(/\/$/, '') ?? ''

export function runtimePublicBaseUrl() { return runtimeBaseUrl || window.location.origin }

export class ApiClientError extends Error {
  status: number
  detail: ApiError
  retryAfterSeconds?: number
  constructor(status: number, detail: ApiError, retryAfterSeconds?: number) { super(apiErrorTranslator?.(detail, status) ?? detail.message); this.status = status; this.detail = detail; this.retryAfterSeconds = retryAfterSeconds }
}

export function setAccessToken(value?: string | null) { accessToken = value ?? undefined }
export function setRefreshHandler(handler: () => Promise<boolean>) { refreshHandler = handler }
export function setApiErrorTranslator(translator?: (detail: ApiError, status: number) => string) { apiErrorTranslator = translator }

export function apiFieldErrors(error: unknown): Record<string, string> {
  if (!(error instanceof ApiClientError)) return {}
  return Object.fromEntries((error.detail.fieldErrors ?? []).map((item) => [item.field, apiErrorTranslator?.({ ...error.detail, code: item.code, message: item.message }, error.status) ?? item.message]))
}

async function parseError(response: Response): Promise<ApiError> {
  try {
    const detail = await response.json() as ApiError
    return { ...detail, requestId: detail.requestId?.trim() || response.headers.get('x-request-id')?.trim() || '' }
  } catch {
    return { code: 'HTTP_ERROR', message: response.statusText, requestId: response.headers.get('x-request-id') ?? '' }
  }
}

async function sendRaw(basePath: string, path: string, init: RequestInit, retry: boolean): Promise<Response> {
  const headers = new Headers(init.headers)
  if (init.body && !(typeof FormData !== 'undefined' && init.body instanceof FormData)) headers.set('Content-Type', 'application/json')
  if (accessToken) headers.set('Authorization', `Bearer ${accessToken}`)
  let response: Response
  try {
    response = await fetch(`${basePath}${path}`, { ...init, credentials: 'include', headers })
  } catch {
    throw new ApiClientError(0, { code: 'NETWORK_ERROR', message: 'Network request failed', requestId: '' })
  }
  if (response.status === 401 && retry && refreshHandler) {
    refreshPromise ??= refreshHandler().finally(() => { refreshPromise = undefined })
    if (await refreshPromise) return sendRaw(basePath, path, init, false)
  }
  if (!response.ok) throw new ApiClientError(response.status, await parseError(response))
  return response
}

async function send<T>(basePath: string, path: string, init: RequestInit, retry: boolean): Promise<T> {
  const response = await sendRaw(basePath, path, init, retry)
  const body = await response.text()
  if (!body) return undefined as T
  return JSON.parse(body) as T
}

export function apiRequest<T>(path: string, init: RequestInit = {}) { return send<T>('/api/v1', path, init, true) }
export async function apiRequestCompleted<T>(path: string, init: RequestInit = {}) {
  const response = await sendRaw('/api/v1', path, init, true)
  if (response.status === 202) {
    const retryAfter = Number.parseInt(response.headers.get('retry-after') ?? '', 10)
    throw new ApiClientError(response.status, await parseError(response), Number.isFinite(retryAfter) ? retryAfter : undefined)
  }
  const body = await response.text()
  return body ? JSON.parse(body) as T : undefined as T
}
export function publicRequest<T>(path: string, init: RequestInit = {}) { return send<T>('/api/v1', path, init, false) }
export function gatewayRequest<T>(path: string, init: RequestInit = {}) { return send<T>(`${runtimeBaseUrl}/gateway/v1`, path, init, true) }
export function gatewayRequestStream(path: string, init: RequestInit = {}) { return sendRaw(`${runtimeBaseUrl}/gateway/v1`, path, init, true) }
export async function apiRequestText(path: string, init: RequestInit = {}) { return (await sendRaw('/api/v1', path, init, true)).text() }
export async function apiRequestBlob(path: string, init: RequestInit = {}) { return (await sendRaw('/api/v1', path, init, true)).blob() }
export const jsonBody = (value: unknown) => JSON.stringify(value)

export async function refreshAccessToken() {
  return publicRequest<AuthResponse>('/auth/refresh', { method: 'POST' })
}
