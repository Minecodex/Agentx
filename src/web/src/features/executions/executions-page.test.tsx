import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../../app/i18n'
import { ToastProvider } from '../../shared/ui/toast'
import { ExecutionsPage } from './executions-page'

const firstExecution = {
  id: '00000000-0000-0000-0000-000000000001',
  applicationId: '00000000-0000-0000-0000-000000000010',
  applicationName: '客服应用',
  workflowId: '00000000-0000-0000-0000-000000000020',
  workflowName: '客服工作流',
  workflowVersionNumber: 3,
  endUser: { source: 'platform', externalUserId: '01a13556-0d5c-7972-b70b-a045766b2171', displayName: '调用用户张三' },
  triggerType: 'schedule',
  triggerName: 'Nightly build',
  initiatorUserId: null,
  initiatorUserName: null,
  initiatorDepartmentId: null,
  initiatorDepartmentName: null,
  status: 'succeeded',
  startedAt: '2026-08-22T01:00:00Z',
  durationMs: 120,
  costMicros: 3,
  costCurrency: 'USD',
}

function renderPage(initialEntry = '/executions') {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  render(
    <QueryClientProvider client={client}>
      <ToastProvider>
        <MemoryRouter initialEntries={[initialEntry]}>
          <ExecutionsPage />
        </MemoryRouter>
      </ToastProvider>
    </QueryClientProvider>,
  )
}

describe('execution server filters', () => {
  let requests: URL[] = []

  beforeEach(async () => {
    requests = []
    await i18n.changeLanguage('zh-CN')
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input), 'http://agentx.test')
      if (url.pathname.endsWith('/mcp/tools')) return jsonResponse({ items: [{ id: 'tool-1', name: 'echo', title: 'Echo Tool' }], page: 1, pageSize: 20, total: 1 })
      if (url.pathname.endsWith('/departments/search')) return jsonResponse({ items: [{ id: 'department-1', name: '研发部' }], page: 1, pageSize: 20, total: 1 })
      if (!url.pathname.endsWith('/executions')) return jsonResponse({ items: [], page: 1, pageSize: 20, total: 0 })
      requests.push(url)
      if (url.searchParams.get('cursor') === 'expired') {
        return jsonResponse({ code: 'QUERY_CURSOR_EXPIRED', message: 'expired', requestId: 'request-1' }, 400)
      }
      return jsonResponse({
        items: url.searchParams.has('cursor') ? [{ ...firstExecution, id: '00000000-0000-0000-0000-000000000002', workflowName: '第二页工作流' }] : [firstExecution],
        limit: 8,
        total: 9,
        nextCursor: url.searchParams.has('cursor') ? null : 'page-2',
      })
    }))
  })

  it('debounces text filters, stores filters in the URL request, and resets cursor pagination', async () => {
    renderPage()
    expect(await screen.findByText('客服工作流')).toBeInTheDocument()
    expect(screen.getByText('调用用户张三')).toBeInTheDocument()
    expect(screen.queryByText(firstExecution.endUser.externalUserId)).not.toBeInTheDocument()
    expect(requests.at(-1)?.searchParams.get('limit')).toBe('8')

    fireEvent.click(screen.getByRole('button', { name: '下一页' }))
    expect(await screen.findByText('第二页工作流')).toBeInTheDocument()
    expect(requests.at(-1)?.searchParams.get('cursor')).toBe('page-2')

    const search = screen.getByPlaceholderText('搜索执行 ID、Trace ID 或错误码')
    fireEvent.change(search, { target: { value: 'TRACE_FAILURE' } })
    expect(requests.at(-1)?.searchParams.get('search')).toBeNull()
    await waitFor(() => expect(requests.at(-1)?.searchParams.get('search')).toBe('TRACE_FAILURE'))
    expect(requests.at(-1)?.searchParams.get('cursor')).toBeNull()
    expect(screen.getByRole('button', { name: '上一页' })).toBeDisabled()

    fireEvent.click(screen.getByRole('button', { name: /更多筛选/ }))
    fireEvent.change(await screen.findByLabelText('触发名称'), { target: { value: 'Nightly' } })
    expect(requests.at(-1)?.searchParams.get('triggerName')).toBeNull()
    await waitFor(() => expect(requests.at(-1)?.searchParams.get('triggerName')).toBe('Nightly'))
    expect(await screen.findByRole('button', { name: /触发名称: Nightly/ })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /触发名称: Nightly/ }))
    await waitFor(() => expect(requests.at(-1)?.searchParams.get('triggerName')).toBeNull())
  })

  it('loads paged tool and department options and writes their selections to the URL', async () => {
    renderPage()
    expect(await screen.findByText('客服工作流')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /更多筛选/ }))

    fireEvent.click(screen.getByRole('button', { name: '选择工具' }))
    fireEvent.click(await screen.findByRole('option', { name: /Echo Tool/ }))
    await waitFor(() => expect(requests.at(-1)?.searchParams.get('toolIds')).toBe('tool-1'))

    fireEvent.click(screen.getByRole('button', { name: '选择部门' }))
    fireEvent.click(await screen.findByRole('option', { name: '研发部' }))
    await waitFor(() => expect(requests.at(-1)?.searchParams.get('initiatorDepartmentIds')).toBe('department-1'))

    fireEvent.click(screen.getByRole('button', { name: '清除全部' }))
    await waitFor(() => expect(requests.at(-1)?.searchParams.get('toolIds')).toBeNull())
    expect(requests.at(-1)?.searchParams.get('initiatorDepartmentIds')).toBeNull()
  })

  it('returns to the first page and tells the user when a cursor expires', async () => {
    vi.mocked(fetch).mockImplementation(async (input: RequestInfo | URL) => {
      const url = new URL(String(input), 'http://agentx.test')
      if (url.searchParams.get('cursor') === 'expired') {
        return jsonResponse({ code: 'QUERY_CURSOR_EXPIRED', message: 'expired', requestId: 'request-1' }, 400)
      }
      return jsonResponse({ items: [firstExecution], limit: 8, total: 9, nextCursor: 'expired' })
    })
    renderPage()
    expect(await screen.findByText('客服工作流')).toBeInTheDocument()
    await waitFor(() => expect(screen.getByRole('button', { name: '下一页' })).toBeEnabled())
    fireEvent.click(screen.getByRole('button', { name: '下一页' }))
    expect(await screen.findByText('查询游标已过期，结果已刷新')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '上一页' })).toBeDisabled()
  })
})

function jsonResponse(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } })
}
