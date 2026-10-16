import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../../app/i18n'
import { useAuth } from '../../app/providers/auth-provider'
import { ToastProvider } from '../../shared/ui/toast'
import { WorkflowDetailPage } from './workflow-detail-page'

vi.mock('../../app/providers/auth-provider', () => ({ useAuth: vi.fn() }))

describe('workflow release experience', () => {
  let requests: Array<{ path: string; init?: RequestInit }>
  let workflow: typeof workflowResponse
  let versions: Array<typeof version1>
  let deployments: Array<typeof deployment1>

  beforeEach(async () => {
    requests = []
    workflow = { ...workflowResponse }
    versions = [{ ...version2 }, { ...version1 }]
    deployments = [{ ...deployment1 }]
    await i18n.changeLanguage('zh-CN')
    vi.mocked(useAuth).mockReturnValue({
      status: 'authenticated', user: undefined, changeToken: undefined,
      setup: vi.fn(), login: vi.fn(), changePassword: vi.fn(), logout: vi.fn(),
      hasPermission: () => true,
    })
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = new URL(String(input), 'http://agentx.test').pathname
      requests.push({ path, init })
      if (path === '/api/v1/workflows/workflow-1' && !init?.method) return jsonResponse(workflow)
      if (path === '/api/v1/workflows/workflow-1/drafts') return jsonResponse([{id:'draft-1',name:'Draft',workflowId:'workflow-1',revision:18,status:'active',baseVersionId:null}])
      if (path === '/api/v1/workflows/workflow-1/versions' && init?.method === 'POST') {
        const created = { ...version2, id: 'version-3', versionNumber: 3, sourceRevision: 18, createdAt: '2026-08-22T03:00:00Z' }
        versions = [created, ...versions]
        workflow = { ...workflow, latestVersion: 3 }
        return jsonResponse(created, 201)
      }
      if (path === '/api/v1/workflows/workflow-1/versions') return jsonResponse(versions)
      if (path === '/api/v1/workflows/workflow-1/deployments' && init?.method === 'POST') {
        const body = JSON.parse(String(init.body)) as { environmentId: string; workflowVersionId: string }
        deployments = deployments.map((item) => item.environmentId === body.environmentId && item.status === 'active' ? { ...item, status: 'superseded' } : item)
        const version = versions.find((item) => item.id === body.workflowVersionId)!
        const created = { ...deployment1, id: `deployment-${deployments.length + 1}`, workflowVersionId: version.id, versionNumber: version.versionNumber, sequenceNumber: 2, createdAt: '2026-08-22T03:01:00Z' }
        deployments = [created, ...deployments]
        return jsonResponse(created, 201)
      }
      if (path === '/api/v1/workflows/workflow-1/deployments') return jsonResponse(deployments)
      if (path === '/api/v1/environments') return jsonResponse(environments)
      if (path === '/api/v1/workflows/workflow-1/members') return jsonResponse([{ userId: 'user-1', username: 'kakj', displayName: 'KA', memberRole: 'owner' }])
      if (path === '/api/v1/workflows/workflow-1/drafts/draft-1/resource-validation') return jsonResponse({ valid: true, missingGrants: [] })
      if (path === '/api/v1/users') return jsonResponse({ items: [], page: 1, pageSize: 100, total: 0 })
      return jsonResponse({})
    }))
  })

  it('shows draft, immutable version, and environment state as separate concepts', async () => {
    await renderPage()
    expect(await screen.findByRole('heading', { name: '订单审核助手' })).toBeInTheDocument()
    expect(screen.getByText('草稿有未发布变更')).toBeInTheDocument()
    expect(screen.getByText('当前草稿')).toBeInTheDocument()
    expect(screen.getByText('最新不可变版本')).toBeInTheDocument()
    expect(screen.getByText('Development 当前版本')).toBeInTheDocument()
    expect(screen.getByText('Development 当前')).toBeInTheDocument()
    expect(screen.getByText('未发布')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '保存为版本' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '查看发布记录' })).toBeInTheDocument()
  })

  it('requires choosing a draft for the header canvas action and keeps each draft link usable', async () => {
    await renderPage(false)
    expect(await screen.findByRole('button', { name: '打开画布' })).toBeDisabled()
    expect(screen.getByRole('link', { name: '打开画布' })).toHaveAttribute('href', '/workflows/workflow-1/drafts/draft-1/editor')
    fireEvent.click(screen.getByRole('combobox', { name: '选择当前草稿' }))
    fireEvent.click(await screen.findByRole('option', { name: 'Draft · r18' }))
    expect(screen.getByRole('button', { name: '打开画布' })).toBeEnabled()
  })

  it('saves the current draft as a version before publishing it to an environment', async () => {
    await renderPage()
    fireEvent.click(await screen.findByRole('button', { name: '发布到环境…' }))
    const dialog = await screen.findByRole('dialog', { name: '发布到环境…' })
    expect(within(dialog).getByRole('combobox', { name: '发布内容' })).toHaveTextContent('当前草稿 r18（自动保存为 v3）')
    fireEvent.click(within(dialog).getByRole('button', { name: '发布到环境…' }))

    await waitFor(() => expect(requests.filter((item) => item.init?.method === 'POST').length).toBe(2))
    const posts = requests.filter((item) => item.init?.method === 'POST')
    expect(posts.map((item) => item.path)).toEqual(['/api/v1/workflows/workflow-1/versions', '/api/v1/workflows/workflow-1/deployments'])
    expect(JSON.parse(String(posts[1].init?.body))).toEqual({ environmentId: 'environment-dev', workflowVersionId: 'version-3' })
    await waitFor(() => expect(screen.queryByRole('dialog', { name: '发布到环境…' })).not.toBeInTheDocument())
    await waitFor(() => expect(screen.queryByText('草稿有未发布变更')).not.toBeInTheDocument())
    expect(screen.getByText('当前 v3 · 部署 #2')).toBeInTheDocument()
  })

  it('blocks publishing the version already active in the selected environment', async () => {
    await renderPage()
    const v1 = (await screen.findAllByText(/^v1$/)).find((element) => element.classList.contains('text-xs'))!
    const row = v1.closest('div.flex.flex-wrap.items-center.gap-3.py-4') as HTMLElement
    fireEvent.click(within(row).getByRole('button', { name: '发布到…' }))
    const dialog = await screen.findByRole('dialog', { name: '发布到环境…' })
    expect(within(dialog).getByText('v1 已经是 Development 的当前版本，无需重复发布。')).toBeInTheDocument()
    expect(within(dialog).getByRole('button', { name: '发布到环境…' })).toBeDisabled()
  })
})

async function renderPage(selectDraft = true) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  const result = render(<QueryClientProvider client={queryClient}><ToastProvider><MemoryRouter initialEntries={['/workflows/workflow-1']}><Routes><Route element={<WorkflowDetailPage />} path="/workflows/:workflowId" /></Routes></MemoryRouter></ToastProvider></QueryClientProvider>)
  if (selectDraft) {
    fireEvent.click(await screen.findByRole('combobox', {name:'选择当前草稿'}))
    fireEvent.click(await screen.findByRole('option', {name:'Draft · r18'}))
  }
  return result
}

const workflowResponse = {
  id: 'workflow-1', name: '订单审核助手', description: '设计、测试并发布企业智能体工作流。', status: 'active', visibility: 'private',
  ownerUserId: 'user-1', ownerDepartmentId: 'department-1', ownerName: 'kakj', serviceIdentityId: 'identity-1',
  canEdit: true, version: 1, draftCount: 1, latestVersion: 2, updatedAt: '2026-08-22T02:00:00Z',
}

const version1 = {
  id: 'version-1', workflowId: 'workflow-1', versionNumber: 1, sourceRevision: 12, schemaVersion: '8.0', contentHash: 'sha256:version-one',
  sourceDraftId:'draft-1',sourceRevisionId:'revision-1',baseVersionId:null,definition: {}, editorDocument: {}, createdBy: 'user-1', createdAt: '2026-08-21T09:00:00Z',
}

const version2 = {
  ...version1, id: 'version-2', versionNumber: 2, sourceRevision: 16, contentHash: 'sha256:version-two', createdAt: '2026-08-22T01:00:00Z',
}

const deployment1 = {
  id: 'deployment-1', workflowId: 'workflow-1', environmentId: 'environment-dev', environmentName: 'Development', workflowVersionId: 'version-1',
  versionNumber: 1, sequenceNumber: 1, status: 'active', source: 'publish', createdAt: '2026-08-22T02:00:00Z',
}

const environments = [
  { id: 'environment-dev', code: 'development', name: 'Development', isBuiltin: true, status: 'active', version: 1 },
  { id: 'environment-prod', code: 'production', name: 'Production', isBuiltin: false, status: 'active', version: 1 },
]

function jsonResponse(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } })
}
