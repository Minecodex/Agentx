import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../../app/i18n'
import { useAuth } from '../../app/providers/auth-provider'
import { ToastProvider } from '../../shared/ui/toast'
import { ModelDetailPage } from './model-detail-page'

vi.mock('../../app/providers/auth-provider', () => ({ useAuth: vi.fn() }))

describe('model edit form', () => {
  let requests: Array<{ path: string; init?: RequestInit }> = []

  beforeEach(async () => {
    requests = []
    await i18n.changeLanguage('zh-CN')
    vi.mocked(useAuth).mockReturnValue({
      sessionId: 1,
      status: 'authenticated', user: undefined, changeToken: undefined,
      setup: vi.fn(), login: vi.fn(), changePassword: vi.fn(), logout: vi.fn(),
      hasPermission: () => true,
    })
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      requests.push({ path, init })
      if (path === '/api/v1/models/aliases/model-1/test-connection') return jsonResponse({ checkedAt: '2026-08-11T01:00:00Z', status: 'unhealthy', latencyMs: 42, errorCode: 'PROVIDER_HTTP_ERROR', errorMessage: 'provider returned 401 Unauthorized' })
      if (path === '/api/v1/models/aliases/model-1') return jsonResponse(modelResponse)
      if (path === '/api/v1/departments') return jsonResponse([{ id: 'department-1', name: '研发部' }])
      if (path.includes('/prices')) return jsonResponse([{ id: 'price-1', deploymentId: 'deployment-1', versionNumber: 2, currency: 'CNY', inputPerMillion: '3.25000000', outputPerMillion: '7.50000000', createdAt: '2026-08-11T00:00:00Z' }])
      if (path.includes('/deployment-history')) return jsonResponse([])
      return jsonResponse({ items: [], page: 1, pageSize: 100, total: 0 })
    }))
  })

  it('places pricing before department and sends entered prices on edit', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(<QueryClientProvider client={queryClient}><ToastProvider><MemoryRouter initialEntries={['/models/model-1']}><Routes><Route element={<ModelDetailPage />} path="/models/:id" /></Routes></MemoryRouter></ToastProvider></QueryClientProvider>)

    const editButton = await screen.findByRole('button', { name: '编辑模型' })
    await waitFor(() => expect(editButton).toBeEnabled())
    fireEvent.click(editButton)
    const dialog = await screen.findByRole('dialog')
    const fieldLabels = Array.from(dialog.querySelectorAll('form > label > span')).map((element) => element.textContent)
    expect(fieldLabels.slice(6, 13)).toEqual([
      '最大输入 Token*', '最大输出 Token*', '模型状态*', '币种*',
      '输入单价/百万 Token*', '输出单价/百万 Token*', '所属部门*',
    ])

    expect(within(dialog).getByLabelText('API 格式')).toHaveTextContent('OpenAI Chat Completions')
    expect(within(dialog).getByLabelText('币种')).toHaveValue('CNY')
    expect(within(dialog).getByLabelText('输入单价/百万 Token')).toHaveValue(3.25)
    expect(within(dialog).getByLabelText('输出单价/百万 Token')).toHaveValue(7.5)
    fireEvent.change(within(dialog).getByLabelText('输入单价/百万 Token'), { target: { value: '3.75' } })
    fireEvent.change(within(dialog).getByLabelText('输出单价/百万 Token'), { target: { value: '6.5' } })
    fireEvent.click(within(dialog).getByRole('button', { name: '保存' }))

    await waitFor(() => expect(requests.some((request) => request.path === '/api/v1/models/aliases/model-1' && request.init?.method === 'PATCH')).toBe(true))
    const request = requests.find((item) => item.path === '/api/v1/models/aliases/model-1' && item.init?.method === 'PATCH')
    expect(JSON.parse(String(request?.init?.body))).toMatchObject({ providerType: 'openai_compatible', price: { currency: 'CNY', inputPerMillion: '3.75', outputPerMillion: '6.5' } })
  })

  it('shows connection-test error details returned by the server', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(<QueryClientProvider client={queryClient}><ToastProvider><MemoryRouter initialEntries={['/models/model-1']}><Routes><Route element={<ModelDetailPage />} path="/models/:id" /></Routes></MemoryRouter></ToastProvider></QueryClientProvider>)

    fireEvent.click(await screen.findByRole('button', { name: '测试连接' }))

    const alert = await screen.findByRole('alert')
    expect(alert).toHaveTextContent('PROVIDER_HTTP_ERROR')
    expect(alert).toHaveTextContent('provider returned 401 Unauthorized')
    expect(screen.getByText('42 ms')).toBeInTheDocument()
  })

  it('clears a stale connection result after the deployment configuration changes', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(<QueryClientProvider client={queryClient}><ToastProvider><MemoryRouter initialEntries={['/models/model-1']}><Routes><Route element={<ModelDetailPage />} path="/models/:id" /></Routes></MemoryRouter></ToastProvider></QueryClientProvider>)

    fireEvent.click(await screen.findByRole('button', { name: '测试连接' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('PROVIDER_HTTP_ERROR')

    const editButton = screen.getByRole('button', { name: '编辑模型' })
    await waitFor(() => expect(editButton).toBeEnabled())
    fireEvent.click(editButton)
    fireEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: '保存' }))

    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(screen.getByText('未测试')).toBeInTheDocument()
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(screen.queryByText('42 ms')).not.toBeInTheDocument()
  })
})

const modelResponse = {
  id: 'model-1', alias: 'gpt-test', status: 'active', aliasVersion: 1,
  deploymentId: 'deployment-1', connectionName: 'test-connection', providerType: 'openai_compatible',
  endpoint: 'https://example.com/v1', credentialId: null, ownerDepartmentId: 'department-1',
  modelName: 'gpt-test-upstream', maxInputTokens: 128_000, maxOutputTokens: 8_192,
  defaultParameters: {}, revisionNumber: 1, connectionStatus: 'untested', connectionCheckedAt: null,
  updatedAt: '2026-08-11T00:00:00Z',
}

function jsonResponse(value: unknown) {
  return new Response(JSON.stringify(value), { headers: { 'Content-Type': 'application/json' } })
}
