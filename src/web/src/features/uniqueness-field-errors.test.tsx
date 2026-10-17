import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import * as TooltipPrimitive from '@radix-ui/react-tooltip'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import type { ReactElement } from 'react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../app/i18n'
import { useAuth } from '../app/providers/auth-provider'
import { ToastProvider } from '../shared/ui/toast'
import { ApplicationsPage } from './applications/applications-page'
import { DatasetDetailPage } from './datasets/dataset-detail-page'
import { QueryForm } from './knowledge/query-form'
import { McpServersPage } from './mcp/mcp-servers-page'
import { MemoryPage } from './memory/memory-page'
import { ModelsPage } from './models/models-page'
import { OrganizationPage } from './organization/organization-page'
import { RolesPage } from './roles/roles-page'
import { SandboxProfilesPage } from './sandbox-profiles/sandbox-profiles-page'
import { SkillsPage } from './skills/skills-page'
import { EnvironmentsPage } from './workflows/environments-page'

vi.mock('../app/providers/auth-provider', () => ({ useAuth: vi.fn() }))

type FetchHandler = (path: string, init?: RequestInit) => Response | Promise<Response>

const rootDepartment = { id: 'department-1', tenantId: 'tenant-1', parentId: null, name: '公司', path: '/company', depth: 0, isRoot: true, version: 1 }
const emptyPage = { items: [], page: 1, pageSize: 100, total: 0 }

beforeEach(async () => {
  await i18n.changeLanguage('zh-CN')
  vi.mocked(useAuth).mockReturnValue({
    status: 'authenticated', user: {
      id: 'user-1', username: 'admin', displayName: 'Admin', companyId: 'company-1', companyName: 'Company',
      departmentId: rootDepartment.id, departmentName: rootDepartment.name, roles: [], permissions: [], locale: 'zh-CN', timezone: 'Asia/Shanghai',
    }, changeToken: undefined,
    setup: vi.fn(), login: vi.fn(), changePassword: vi.fn(), logout: vi.fn(),
    hasPermission: () => true,
  })
})

describe('resource page uniqueness field errors', () => {
  it('shows and focuses model name conflicts', async () => {
    stubFetch((path, init) => {
      if (path.endsWith('/departments')) return jsonResponse([rootDepartment])
      if (path === '/api/v1/models/aliases' && init?.method === 'POST') return fieldConflict('MODEL_NAME_EXISTS', 'alias')
      return jsonResponse(emptyPage)
    })
    renderPage(<ModelsPage />)
    fireEvent.click(await screen.findByRole('button', { name: '新建模型' }))
    const form = await screen.findByRole('dialog')
    fireEvent.change(within(form).getByLabelText('连接名称'), { target: { value: 'connection' } })
    fireEvent.change(within(form).getByLabelText('Endpoint'), { target: { value: 'https://example.test/v1' } })
    fireEvent.change(within(form).getByLabelText('输入单价/百万 Token'), { target: { value: '1' } })
    fireEvent.change(within(form).getByLabelText('输出单价/百万 Token'), { target: { value: '2' } })
    choose(form, '所属部门', '公司')
    fireEvent.click(within(form).getByRole('button', { name: '保存' }))
    const input = await within(form).findByLabelText('模型名称')
    expect(await within(form).findByText('该模型名称已存在。')).toBeInTheDocument()
    await waitFor(() => expect(input).toHaveFocus())
    expect(input).toHaveAttribute('aria-invalid', 'true')
  })

  it('shows department and username conflicts in the custom organization forms', async () => {
    stubFetch((path, init) => {
      if (path.endsWith('/departments') && init?.method === 'POST') return fieldConflict('DEPARTMENT_NAME_EXISTS', 'name')
      if (path.endsWith('/users') && init?.method === 'POST') return fieldConflict('USERNAME_EXISTS', 'username')
      if (path.endsWith('/departments')) return jsonResponse([rootDepartment])
      if (path.includes('/users?')) return jsonResponse(emptyPage)
      if (path.includes('/roles?')) return jsonResponse({ ...emptyPage, items: [{ id: 'role-1', code: 'member', name: '成员', isBuiltin: true, dataScope: 'own', permissions: [], memberCount: 0, status: 'active', version: 1 }] })
      return jsonResponse(emptyPage)
    })
    renderPage(<OrganizationPage />)

    const departmentButton = await screen.findByRole('button', { name: '新建部门' })
    await waitFor(() => expect(departmentButton).toBeEnabled())
    fireEvent.click(departmentButton)
    let form = await screen.findByRole('dialog')
    fireEvent.change(within(form).getByLabelText('名称'), { target: { value: '公司' } })
    fireEvent.click(within(form).getByRole('button', { name: '保存' }))
    let input = within(form).getByLabelText('名称')
    expect(await within(form).findByText('所选上级部门下已存在同名部门。')).toBeInTheDocument()
    await waitFor(() => expect(input).toHaveFocus())
    fireEvent.click(within(form).getByRole('button', { name: '取消' }))

    fireEvent.click(screen.getByRole('button', { name: '创建用户' }))
    form = await screen.findByRole('dialog', { name: '创建用户' })
    fireEvent.change(within(form).getByLabelText('用户名'), { target: { value: 'admin' } })
    fireEvent.change(within(form).getByLabelText('用户名称'), { target: { value: 'Another admin' } })
    fireEvent.click(within(form).getByRole('button', { name: '保存' }))
    input = within(form).getByLabelText('用户名')
    expect(await within(form).findByText('该用户名已被使用。')).toBeInTheDocument()
    await waitFor(() => expect(input).toHaveFocus())
  })

  it('shows and focuses role code conflicts in the custom role form', async () => {
    stubFetch((path, init) => {
      if (path.endsWith('/roles') && init?.method === 'POST') return fieldConflict('ROLE_CODE_EXISTS', 'code')
      if (path.endsWith('/permissions')) return jsonResponse([])
      return jsonResponse(emptyPage)
    })
    renderPage(<RolesPage />)
    fireEvent.click(await screen.findByRole('button', { name: '新建角色' }))
    const form = await screen.findByRole('dialog', { name: '新建角色' })
    fireEvent.change(within(form).getByLabelText('名称'), { target: { value: 'Operator' } })
    fireEvent.change(within(form).getByLabelText('角色标识'), { target: { value: 'operator' } })
    fireEvent.click(within(form).getByRole('button', { name: '保存' }))
    const input = within(form).getByLabelText('角色标识')
    expect(await within(form).findByText('该角色编码已存在。')).toBeInTheDocument()
    await waitFor(() => expect(input).toHaveFocus())
  })

  it('shows application slug conflicts', async () => {
    stubFetch((path, init) => {
      if (path.endsWith('/applications') && init?.method === 'POST') return fieldConflict('APPLICATION_SLUG_EXISTS', 'slug')
      if (path.includes('/workflows?')) return jsonResponse({ ...emptyPage, items: [{ id: 'workflow-1', name: 'Workflow' }] })
      return jsonResponse(emptyPage)
    })
    renderPage(<ApplicationsPage />)
    const create = await screen.findByRole('button', { name: '新建应用' })
    await waitFor(() => expect(create).toBeEnabled())
    fireEvent.click(create)
    const form = await screen.findByRole('dialog')
    choose(form, '工作流', 'Workflow')
    fireEvent.change(within(form).getByLabelText('名称'), { target: { value: 'App' } })
    fireEvent.change(within(form).getByLabelText('Slug'), { target: { value: 'app' } })
    fireEvent.click(within(form).getByRole('button', { name: '保存' }))
    expect(await within(form).findByText('该应用 Slug 已存在。')).toBeInTheDocument()
    expect(form.querySelector('[name="slug"]')).toHaveAttribute('aria-invalid', 'true')
  })

  it('shows MCP name conflicts', async () => {
    stubFetch((path, init) => {
      if (path.endsWith('/mcp/servers') && init?.method === 'POST') return fieldConflict('MCP_SERVER_NAME_EXISTS', 'name')
      if (path.endsWith('/departments')) return jsonResponse([rootDepartment])
      return jsonResponse(emptyPage)
    })
    renderPage(<McpServersPage />)
    fireEvent.click(await screen.findByRole('button', { name: '接入 MCP 服务' }))
    const form = await screen.findByRole('dialog')
    fireEvent.change(within(form).getByLabelText('名称'), { target: { value: 'MCP' } })
    choose(form, '所属部门', '公司')
    fireEvent.click(within(form).getByRole('button', { name: '保存' }))
    expect(await within(form).findByText('该 MCP Server 名称已存在。')).toBeInTheDocument()
  })

  it('shows both Skill identity conflict fields', async () => {
    stubFetch((path, init) => {
      if (path.endsWith('/skills') && init?.method === 'POST') return fieldConflict('SKILL_ALIAS_EXISTS', 'alias')
      if (path.endsWith('/departments')) return jsonResponse([rootDepartment])
      return jsonResponse(emptyPage)
    })
    renderPage(<SkillsPage />)
    fireEvent.click(await screen.findByRole('button', { name: '新建技能' }))
    const form = await screen.findByRole('dialog')
    fireEvent.change(within(form).getByLabelText('技能名称'), { target: { value: 'Skill' } })
    fireEvent.change(within(form).getByLabelText('技能别名'), { target: { value: 'skill' } })
    fireEvent.change(within(form).getByLabelText('技能描述'), { target: { value: 'Description' } })
    choose(form, '所属部门', '公司')
    fireEvent.click(within(form).getByRole('button', { name: '保存' }))
    expect(await within(form).findByText('该技能别名已存在。')).toBeInTheDocument()
    expect(form.querySelector('[name="alias"]')).toHaveAttribute('aria-invalid', 'true')
  })

  it('shows Sandbox Profile name conflicts', async () => {
    stubFetch((path, init) => {
      if (path.endsWith('/sandbox-profiles') && init?.method === 'POST') return fieldConflict('SANDBOX_PROFILE_NAME_EXISTS', 'name')
      if (path.endsWith('/departments')) return jsonResponse([rootDepartment])
      return jsonResponse(emptyPage)
    })
    renderPage(<SandboxProfilesPage />)
    fireEvent.click(await screen.findByRole('button', { name: '新建沙箱配置' }))
    const form = await screen.findByRole('dialog')
    fireEvent.change(within(form).getByLabelText('名称'), { target: { value: 'Default' } })
    choose(form, '所属部门', '公司')
    fireEvent.change(within(form).getByLabelText('镜像 Digest'), { target: { value: 'opensandbox/code-interpreter@sha256:133a3c1720dd52291a019740c2987e7164ea6de79e23d8198798e58950ae2e6e' } })
    fireEvent.click(within(form).getByRole('button', { name: '保存' }))
    expect(await within(form).findByText('该沙箱配置名称已存在。')).toBeInTheDocument()
  })

  it('shows Dataset Case Key conflicts', async () => {
    stubFetch((path, init) => {
      if (path.endsWith('/datasets/dataset-1/cases') && init?.method === 'POST') return fieldConflict('DATASET_CASE_KEY_EXISTS', 'caseKey')
      if (path.endsWith('/datasets/dataset-1')) return jsonResponse({ id: 'dataset-1', name: 'Dataset', description: null, visibility: 'department', status: 'active', ownerUserId: 'user-1', ownerDepartmentId: 'department-1', revision: 1, latestVersion: null, caseCount: 0, version: 1, updatedAt: '2026-08-12T00:00:00Z' })
      if (path.endsWith('/cases') || path.endsWith('/versions')) return jsonResponse([])
      return jsonResponse(emptyPage)
    })
    renderPage(<Routes><Route element={<DatasetDetailPage />} path="/datasets/:id" /></Routes>, '/datasets/dataset-1')
    fireEvent.click(await screen.findByRole('button', { name: '新增用例' }))
    const form = await screen.findByRole('dialog')
    fireEvent.change(within(form).getByLabelText('用例标识'), { target: { value: 'case-1' } })
    fireEvent.change(within(form).getByLabelText('名称'), { target: { value: 'Case' } })
    fireEvent.click(within(form).getByRole('button', { name: '保存' }))
    expect(await within(form).findByText('测试集中已存在相同的用例 Key。')).toBeInTheDocument()
  })

  it('shows a query name conflict within its connection', async () => {
    stubFetch((path, init) => {
      if (path.endsWith('/catalog')) return jsonResponse([{ id: 'dataset-a', name: 'Product documentation', scope: { kind: 'ragflow', datasetIds: ['dataset-a'] } }])
      if (path.endsWith('/queries') && init?.method === 'POST') return fieldConflict('KNOWLEDGE_NAME_EXISTS', 'name')
      if (path.endsWith('/departments')) return jsonResponse([rootDepartment])
      return jsonResponse([])
    })
    renderPage(<QueryForm connection={{ id: 'connection-1', name: 'Docs', provider: 'ragflow', endpoint: 'https://kb.example', credentialId: null, ownerDepartmentId: rootDepartment.id, status: 'active', version: 1, healthPath: '/health', configuration: {} }} onClose={() => undefined} onSaved={async () => undefined} />)
    const form = await screen.findByRole('dialog')
    fireEvent.change(within(form).getByLabelText('查询名称'), { target: { value: 'Product query' } })
    fireEvent.click(await within(form).findByRole('checkbox', { name: /Product documentation/ }))
    fireEvent.click(within(form).getByRole('button', { name: '保存' }))
    expect(await within(form).findByText('同一连接下已存在相同的查询名称。')).toBeInTheDocument()
  })

  it('creates a single read/write Memory connection without namespace fields', async () => {
    let submitted: Record<string, unknown> | undefined
    stubFetch((path, init) => {
      if (path.endsWith('/memory/connections') && init?.method === 'POST') {
        submitted = JSON.parse(String(init.body))
        return jsonResponse({ id: 'memory-1' }, 201)
      }
      if (path.endsWith('/departments')) return jsonResponse([rootDepartment])
      return jsonResponse([])
    })
    renderPage(<MemoryPage />)
    fireEvent.click(await screen.findByRole('button', { name: '接入记忆服务' }))
    const form = await screen.findByRole('dialog')
    for (const label of ['资源名称', '记忆命名空间', '访问模式']) expect(within(form).queryByLabelText(label)).not.toBeInTheDocument()
    fireEvent.change(within(form).getByLabelText('连接名称'), { target: { value: 'Mem0 connection' } })
    fireEvent.change(within(form).getByLabelText('Endpoint'), { target: { value: 'https://mem0.example.test' } })
    choose(form, '所属部门', '公司')
    fireEvent.click(within(form).getByRole('button', { name: '保存' }))
    await waitFor(() => expect(submitted).toEqual({ name: 'Mem0 connection', endpoint: 'https://mem0.example.test', healthPath: '/health', credentialId: null, ownerDepartmentId: rootDepartment.id, configuration: {} }))
  })

  it('shows Workflow environment code conflicts', async () => {
    stubFetch((path, init) => path.endsWith('/environments') && init?.method === 'POST' ? fieldConflict('ENVIRONMENT_CODE_EXISTS', 'code') : jsonResponse([]))
    renderPage(<EnvironmentsPage />)
    fireEvent.click(await screen.findByRole('button', { name: '新建环境' }))
    const form = await screen.findByRole('dialog')
    fireEvent.change(within(form).getByLabelText('环境标识'), { target: { value: 'staging' } })
    fireEvent.change(within(form).getByLabelText('环境名称'), { target: { value: 'Staging' } })
    fireEvent.click(within(form).getByRole('button', { name: '保存' }))
    expect(await within(form).findByText('该环境编码已存在。')).toBeInTheDocument()
  })
})

function renderPage(element: ReactElement, path = '/') {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  return render(<QueryClientProvider client={queryClient}><TooltipPrimitive.Provider><ToastProvider><MemoryRouter initialEntries={[path]}>{element}</MemoryRouter></ToastProvider></TooltipPrimitive.Provider></QueryClientProvider>)
}

function stubFetch(handler: FetchHandler) {
  vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => handler(new URL(String(input), 'http://agentx.test').pathname + new URL(String(input), 'http://agentx.test').search, init)))
}

function jsonResponse(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } })
}

function fieldConflict(code: string, field: string) {
  return jsonResponse({ code, message: 'duplicate', requestId: 'request-1', fieldErrors: [{ field, code, message: 'duplicate' }] }, 409)
}

function choose(container: HTMLElement, label: string, option: string) {
  fireEvent.click(within(container).getByRole('combobox', { name: label }))
  fireEvent.click(screen.getByRole('option', { name: option }))
}
