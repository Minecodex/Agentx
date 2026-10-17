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
  let environmentItems: typeof environments
  let environmentFailure: boolean

  beforeEach(async () => {
    requests = []
    workflow = { ...workflowResponse }
    versions = [{ ...version2 }, { ...version1 }]
    deployments = [{ ...deployment1 }]
    environmentItems = [...environments]
    environmentFailure = false
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
      if (path === '/api/v1/environments') return environmentFailure ? jsonResponse({message:'Environment service unavailable'}, 503) : jsonResponse(environmentItems)
      if (path === '/api/v1/workflows/workflow-1/members') return jsonResponse([{ userId: 'user-1', username: 'kakj', displayName: 'KA', memberRole: 'owner' }])
      if (path.endsWith('/member-candidates')) return jsonResponse([])
      return jsonResponse({})
    }))
  })

  it('shows version entry points and keeps member management in a dialog', async () => {
    renderPage()
    const row = await screen.findByTestId('workflow-version-2')
    expect(within(row).getByRole('link', { name: '详情' })).toHaveAttribute('href', '/workflows/workflow-1/versions/version-2')
    expect(within(row).getByRole('link', { name: '编辑' })).toHaveAttribute('href', '/workflows/workflow-1/versions/version-2/editor')
    expect(screen.queryByRole('button', { name: '打开画布' })).not.toBeInTheDocument()
    const header = screen.getByRole('heading', { name: workflowResponse.name }).closest('header')!
    expect(within(header).getAllByRole('button').map(button => button.textContent)).toEqual(['成员管理', '新建画布'])
    expect(screen.getByRole('button', { name: '新建画布' })).toBeEnabled()
    expect(requests.some(item => item.path.endsWith('/members'))).toBe(false)
    fireEvent.click(screen.getByRole('button', { name: '成员管理' }))
    const dialog=await screen.findByRole('dialog', {name:'成员管理'})
    expect(await within(dialog).findByText('KA')).toBeInTheDocument()
  })

  it('publishes an exact saved draft in one idempotent request', async () => {
    renderPage()
    const row=await screen.findByTestId('workflow-version-2')
    fireEvent.click(within(row).getByRole('button', {name:'发布'}))
    const dialog=await screen.findByRole('dialog', {name:'发布到环境…'})
    expect(within(dialog).getByRole('combobox', {name:'发布内容'})).toHaveTextContent('r18')
    fireEvent.click(within(dialog).getByRole('button', {name:'发布到环境…'}))
    await waitFor(()=>expect(requests.filter(item=>item.init?.method==='POST')).toHaveLength(1))
    const post=requests.find(item=>item.init?.method==='POST')!
    expect(post.path).toBe('/api/v1/workflows/workflow-1/deployments')
    expect(JSON.parse(String(post.init?.body))).toEqual({environmentId:'environment-dev',workflowVersionId:'version-2',contentSource:'draft',expectedRevision:18})
    expect(new Headers(post.init?.headers).get('Idempotency-Key')).toBeTruthy()
    await waitFor(()=>expect(screen.queryByRole('dialog', {name:'发布到环境…'})).not.toBeInTheDocument())
  })

  it('blocks publishing the original version already active in the target environment', async () => {
    renderPage()
    const row=await screen.findByTestId('workflow-version-1')
    fireEvent.click(within(row).getByRole('button', {name:'发布'}))
    const dialog=await screen.findByRole('dialog', {name:'发布到环境…'})
    expect(within(dialog).getByRole('button', {name:'发布到环境…'})).toBeDisabled()
  })


  it('explains missing environments and keeps the source dialog open when managing them', async () => {
    environmentItems = []
    renderPage()
    fireEvent.click(within(await screen.findByTestId('workflow-version-2')).getByRole('button', { name: '发布' }))
    const dialog = await screen.findByRole('dialog', { name: '发布到环境…' })
    expect(within(dialog).getByRole('combobox', { name: '目标环境' })).toBeDisabled()
    expect(within(dialog).getByText('请先在环境管理中创建或启用环境，再返回这里发布。')).toBeInTheDocument()
    expect(within(dialog).getByRole('link', { name: '管理环境' })).toHaveAttribute('href', '/environments')
    expect(within(dialog).getByRole('link', { name: '管理环境' })).toHaveAttribute('target', '_blank')
    expect(within(dialog).getByRole('button', { name: '发布到环境…' })).toBeDisabled()
    environmentItems = [...environments]
    fireEvent.click(within(dialog).getByRole('button', { name: '重新加载环境' }))
    await waitFor(() => expect(within(dialog).getByRole('combobox', { name: '目标环境' })).toHaveTextContent('Development'))
    expect(within(dialog).getByRole('button', { name: '发布到环境…' })).toBeEnabled()
  })

  it('reports a failed environment query and selects the recovered environment after retry', async () => {
    environmentFailure = true
    renderPage()
    fireEvent.click(within(await screen.findByTestId('workflow-version-2')).getByRole('button', { name: '发布' }))
    const dialog = await screen.findByRole('dialog', { name: '发布到环境…' })
    expect(await within(dialog).findByText('环境加载失败，请重试。')).toBeInTheDocument()
    expect(within(dialog).queryByText('请先在环境管理中创建或启用环境，再返回这里发布。')).not.toBeInTheDocument()
    expect(within(dialog).getByRole('button', { name: '发布到环境…' })).toBeDisabled()
    environmentFailure = false
    fireEvent.click(within(dialog).getByRole('button', { name: '重新加载环境' }))
    await waitFor(() => expect(within(dialog).getByRole('combobox', { name: '目标环境' })).toHaveTextContent('Development'))
    expect(within(dialog).getByRole('button', { name: '发布到环境…' })).toBeEnabled()
  })

  it('keeps archived versions readable while disabling editing', async () => {
    versions[0].archived=true
    renderPage()
    const row=await screen.findByTestId('workflow-version-2')
    expect(within(row).getByRole('button',{name:'编辑'})).toBeDisabled()
    expect(within(row).getByRole('link',{name:'详情'})).toHaveAttribute('href')
    expect(within(row).getByRole('button',{name:'发布'})).toBeEnabled()
  })
})

function renderPage() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  return render(<QueryClientProvider client={queryClient}><ToastProvider><MemoryRouter initialEntries={['/workflows/workflow-1']}><Routes><Route element={<WorkflowDetailPage />} path="/workflows/:workflowId" /></Routes></MemoryRouter></ToastProvider></QueryClientProvider>)
}

const workflowResponse = {
  id: 'workflow-1', name: '订单审核助手', description: '设计、测试并发布企业智能体工作流。', status: 'active', visibility: 'private',
  ownerUserId: 'user-1', ownerDepartmentId: 'department-1', ownerName: 'kakj', serviceIdentityId: 'identity-1',
  canEdit: true, version: 1, draftCount: 1, latestVersion: 2, updatedAt: '2026-08-22T02:00:00Z',
}

const version1 = {
  publicationStatus:'published',archived:false,hasDraft:false,draftRevision:12,updatedByName:'KA',updatedAt:'2026-08-21T09:00:00Z',
  id: 'version-1', workflowId: 'workflow-1', versionNumber: 1, sourceRevision: 12, schemaVersion: '8.0', contentHash: 'sha256:version-one',
  sourceDraftId:'draft-1',sourceRevisionId:'revision-1',baseVersionId:null,definition: {}, editorDocument: {}, createdBy: 'user-1', createdAt: '2026-08-21T09:00:00Z',
}

const version2 = {
  ...version1, hasDraft:true,draftRevision:18,id: 'version-2', versionNumber: 2, sourceRevision: 16, contentHash: 'sha256:version-two', createdAt: '2026-08-22T01:00:00Z',
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
