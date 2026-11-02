import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../../app/i18n'
import { useAuth } from '../../app/providers/auth-provider'
import { ToastProvider } from '../../shared/ui/toast'
import { ModelsPage } from './models-page'

vi.mock('../../app/providers/auth-provider', () => ({ useAuth: vi.fn() }))

describe('model connection form', () => {
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
      if (path.endsWith('/departments')) return jsonResponse([{ id: 'department-1', name: '研发部' }])
      return jsonResponse({ items: [], page: 1, pageSize: 100, total: 0 })
    }))
  })

  it('uses model capability defaults without exposing a deployment name', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(<QueryClientProvider client={queryClient}><ToastProvider><MemoryRouter><ModelsPage /></MemoryRouter></ToastProvider></QueryClientProvider>)

    const createButton = await screen.findByRole('button', { name: '新建模型' })
    await waitFor(() => expect(createButton).toBeEnabled())
    fireEvent.click(createButton)
    const dialog = await screen.findByRole('dialog')

    expect(within(dialog).getByLabelText('模型名称')).toHaveValue('gpt-5.6-sol')
    expect(within(dialog).getByLabelText('上游模型 ID')).toHaveValue('gpt-5.6-sol')
    expect(within(dialog).getByLabelText('最大输入 Token')).toHaveValue(1_050_000)
    expect(within(dialog).getByLabelText('最大输出 Token')).toHaveValue(128_000)
    expect(within(dialog).queryByText(/Deployment 名称/i)).not.toBeInTheDocument()
    expect(within(dialog).getByLabelText('连接名称')).toBeInTheDocument()
    expect(within(dialog).getByLabelText('API 格式')).toHaveTextContent('OpenAI Chat Completions')
    expect(within(dialog).getByLabelText('Endpoint')).toBeInTheDocument()
    expect(within(dialog).getByLabelText('凭证')).toBeInTheDocument()
    expect(within(dialog).getByLabelText('所属部门')).toBeInTheDocument()
    expect(within(dialog).getByLabelText('默认参数 JSON')).toHaveValue('{}')
    const fieldLabels = Array.from(dialog.querySelectorAll('form > label > span')).map((element) => element.textContent)
    expect(fieldLabels.slice(6, 10)).toEqual(['最大输入 Token*', '最大输出 Token*', '币种*', '输入单价/百万 Token*'])
    expect(fieldLabels[10]).toBe('输出单价/百万 Token*')
    expect(fieldLabels[11]).toBe('所属部门*')
    expect(within(dialog).getByLabelText('输入单价/百万 Token')).toBeRequired()
    expect(within(dialog).getByLabelText('输出单价/百万 Token')).toBeRequired()
  })

  it('sends entered prices when creating a model', async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(<QueryClientProvider client={queryClient}><ToastProvider><MemoryRouter><ModelsPage /></MemoryRouter></ToastProvider></QueryClientProvider>)

    fireEvent.click(await screen.findByRole('button', { name: '新建模型' }))
    const dialog = await screen.findByRole('dialog')
    fireEvent.change(within(dialog).getByLabelText('连接名称'), { target: { value: 'test-connection' } })
    fireEvent.change(within(dialog).getByLabelText('Endpoint'), { target: { value: 'https://example.com/v1' } })
    fireEvent.change(within(dialog).getByLabelText('输入单价/百万 Token'), { target: { value: '1.25' } })
    fireEvent.change(within(dialog).getByLabelText('输出单价/百万 Token'), { target: { value: '2.5' } })
    fireEvent.click(within(dialog).getByRole('combobox', { name: '所属部门' }))
    fireEvent.click(await screen.findByRole('option', { name: '研发部' }))
    fireEvent.click(within(dialog).getByRole('button', { name: '保存' }))

    await waitFor(() => expect(requests.some((request) => request.path === '/api/v1/models/aliases' && request.init?.method === 'POST')).toBe(true))
    const request = requests.find((item) => item.path === '/api/v1/models/aliases' && item.init?.method === 'POST')
    expect(JSON.parse(String(request?.init?.body))).toMatchObject({ price: { currency: 'USD', inputPerMillion: '1.25', outputPerMillion: '2.5' } })
  })
})

function jsonResponse(value: unknown) {
  return new Response(JSON.stringify(value), { headers: { 'Content-Type': 'application/json' } })
}
