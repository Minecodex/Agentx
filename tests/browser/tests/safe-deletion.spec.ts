import { beginApplicationEvaluation } from '../support/application-testing'
import { expect, type APIResponse, type Locator, type Page, test } from '@playwright/test'

const adminPassword = 'agentx-e2e-admin-password'
const gatewayBase = process.env.AGENTX_E2E_RUNTIME_URL ?? ''

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill(adminPassword)
  const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/auth/login') && value.request().method() === 'POST')
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
  return ((await (await response).json()) as { accessToken: string }).accessToken
}

async function request<T>(page: Page, token: string, path: string): Promise<T> {
  const response = await page.request.get(`/api/v1${path}`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })
  await expectResponse(response, path)
  return await response.json() as T
}

async function mutate<T>(page: Page, token: string, path: string, method: string, body: unknown, gateway = false): Promise<T> {
  const response = await page.request.fetch(`${gateway ? `${gatewayBase}/gateway/v1` : '/api/v1'}${path}`, {
    method,
    data: body,
    headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': `safe-deletion-${Date.now()}-${Math.random()}` },
  })
  await expectResponse(response, path)
  return await response.json() as T
}

async function expectResponse(response: APIResponse, label: string) {
  if (!response.ok()) throw new Error(`${label}: ${response.status()} ${await response.text()}`)
}

async function field(container: Locator, label: string) {
  return container.locator('label').filter({ hasText: label }).locator('input, textarea').first()
}

async function select(container: Locator, label: string, option: string) {
  await container.getByRole('combobox', { name: label, exact: true }).click()
  await container.page().getByRole('option', { name: option, exact: true }).click()
}

async function openDelete(page: Page, path: string, entityName: string) {
  await page.goto(path)
  const search = page.getByRole('searchbox')
  await expect(search).toBeVisible()
  await search.fill(entityName)
  const row = page.getByRole('row').filter({ hasText: entityName }).first()
  await expect(row).toBeVisible()
  const deleteButton = row.getByRole('button', { name: '删除', exact: true })
  await expect(deleteButton).toHaveText('删除')
  await deleteButton.click()
  const dialog = page.getByRole('dialog', { name: `删除 ${entityName}` })
  await expect(dialog).toBeVisible()
  return { dialog, row }
}

async function expectBlocked(page: Page, path: string, entityName: string) {
  const { dialog } = await openDelete(page, path, entityName)
  await expect(dialog.getByText('该内容仍被引用，无法删除。')).toBeVisible()
  await expect(dialog.getByText(/引用位置 · [1-9]/)).toBeVisible()
  await expect(dialog.getByRole('button', { name: '确认删除' })).toBeDisabled()
  await dialog.getByRole('button', { name: '取消' }).click()
  await expect(dialog).toBeHidden()
}

type WorkflowSummary = { id: string; name: string }
type WorkflowVersion = { id: string; versionNumber: number; definition?: { start?: { inputs?: { properties?: Record<string, { type?: string }> } } } }
type Deployment = { id: string; status: string }

function safeInvocationInput(version: WorkflowVersion) {
  const input: Record<string, unknown> = {}
  const properties = version.definition?.start?.inputs?.properties ?? {}
  for (const [key, schema] of Object.entries(properties)) {
    input[key] = schema.type === 'array' ? [] : schema.type === 'object' ? {} : schema.type === 'boolean' ? true : schema.type === 'number' || schema.type === 'integer' ? 0 : 'safe deletion'
  }
  return input
}

async function createImpactFixtures(page: Page, token: string) {
  const workflows = await request<{ items: WorkflowSummary[] }>(page, token, '/workflows?pageSize=100')
  const workflow = workflows.items.find((item) => item.name === 'MCP Echo Workflow') ?? workflows.items[0]
  if (!workflow) throw new Error('A workflow fixture is required for safe deletion impact tests')
  const versions = await request<WorkflowVersion[]>(page, token, `/workflows/${workflow.id}/versions`)
  const selectedVersion = versions.find((item) => item.versionNumber === 1) ?? versions[0]
  if (!selectedVersion) throw new Error('A workflow version fixture is required for safe deletion impact tests')
  const version = await request<WorkflowVersion>(page, token, `/workflows/${workflow.id}/versions/${selectedVersion.id}`)
  const environments = await request<Array<{ id: string; code: string }>>(page, token, '/environments')
  const environment = environments.find((item) => item.code === 'development') ?? environments[0]
  if (!environment) throw new Error('An environment fixture is required for safe deletion impact tests')

  const suffix = `${Date.now()}`
  const application = await mutate<{ id: string; name: string; slug: string }>(page, token, '/applications', 'POST', { workflowId: workflow.id, name: `Safe Deletion Application ${suffix}`, slug: `safe-deletion-${suffix}`, description: 'Safe deletion E2E fixture', visibility: 'company' })
  const deployment = await mutate<Deployment>(page, token, `/applications/${application.id}/deployments`, 'POST', { workflowVersionId: version.id, environmentId: environment.id, sessionVersionPolicy: 'pinned' })
  await expect.poll(async () => {
    const deployments = await request<Deployment[]>(page, token, `/applications/${application.id}/deployments`)
    const status = deployments.find((item) => item.id === deployment.id)?.status
    if (status === 'rejected') throw new Error(`Safe deletion Application deployment ${deployment.id} was rejected`)
    return status
  }, { timeout: 180_000, intervals: [500, 1_000, 2_000] }).toBe('active')
  const session = await mutate<{ id: string }>(page, token, `/applications/${application.slug}/sessions`, 'POST', { title: 'Safe deletion session' }, true)
  const invocation = await mutate<{ id: string }>(page, token, `/applications/${application.slug}/invocations`, 'POST', { input: safeInvocationInput(version), sessionId: session.id }, true)
  let referenceOutput: unknown
  await expect.poll(async () => {
    const response = await page.request.get(`${gatewayBase}/gateway/v1/invocations/${invocation.id}`, { headers: { Authorization: `Bearer ${token}` } })
    expect(response.ok()).toBeTruthy()
    const value = await response.json() as { status: string; outputs?: unknown }
    referenceOutput = value.outputs
    return value.status
  }, { timeout: 120_000 }).toBe('completed')

  const evaluation = await beginApplicationEvaluation(page, token, { applicationId: application.id, workflowId: workflow.id, versionId: version.id, name: `Safe Deletion Dataset ${suffix}`, input: safeInvocationInput(version), expectedOutput: referenceOutput })
  const dataset = evaluation.dataset
  return { application, dataset, workflow }
}

test('safe deletion blocks references, deletes clear resources, and protects system entities', async ({ page }) => {
  await login(page)

  await expectBlocked(page, '/credentials', 'Echo Credential Renamed')
  await expectBlocked(page, '/models', 'echo-chat')
  await expectBlocked(page, '/mcp', 'Echo MCP')
  await expectBlocked(page, '/skills', 'Workspace Skill')

  await page.goto('/credentials')
  await page.getByRole('button', { name: '新建凭证' }).click()
  const credential = page.getByRole('dialog', { name: '新建凭证' })
  await (await field(credential, '名称')).fill('Disposable Credential')
  await select(credential, '凭证类型', 'API Key')
  await (await field(credential, '密钥内容')).fill('temporary-e2e-secret')
  await select(credential, '所属部门', 'Agentx E2E')
  await credential.getByRole('button', { name: '保存', exact: true }).click()
  await expect(credential).toBeHidden()

  let deletion = await openDelete(page, '/credentials', 'Disposable Credential')
  await expect(deletion.dialog.getByText('删除后无法恢复。').last()).toBeVisible()
  await expect(deletion.dialog.getByRole('button', { name: '确认删除' })).toBeEnabled()
  await deletion.dialog.getByRole('button', { name: '取消' }).click()
  await expect(page.getByRole('row').filter({ hasText: 'Disposable Credential' })).toBeVisible()

  deletion = await openDelete(page, '/credentials', 'Disposable Credential')
  await deletion.dialog.getByRole('button', { name: '确认删除' }).click()
  await expect(deletion.dialog).toBeHidden()
  await expect(page.getByRole('row').filter({ hasText: 'Disposable Credential' })).toHaveCount(0)

  await page.goto('/environments')
  await expect(page.getByRole('button', { name: '系统内置内容不可删除' }).first()).toBeDisabled()
  await page.getByRole('button', { name: '新建环境' }).click()
  const environment = page.getByRole('dialog', { name: '新建环境' })
  await (await field(environment, '环境标识')).fill('disposable')
  await (await field(environment, '环境名称')).fill('Disposable Environment')
  await environment.getByRole('button', { name: '保存', exact: true }).click()
  await expect(environment).toBeHidden()
  deletion = await openDelete(page, '/environments', 'Disposable Environment')
  await deletion.dialog.getByRole('button', { name: '确认删除' }).click()
  await expect(deletion.dialog).toBeHidden()
  await expect(page.getByRole('row').filter({ hasText: 'Disposable Environment' })).toHaveCount(0)

  await page.goto('/roles')
  await expect(page.getByRole('button', { name: '系统内置内容不可删除' }).first()).toBeDisabled()
  await page.goto('/organization')
  await expect(page.getByRole('button', { name: '系统内置内容不可删除' }).first()).toBeDisabled()
})

test('safe deletion exposes application, evaluation and immutable-version impact in the UI', async ({ page }) => {
  const token = await login(page)
  const fixtures = await createImpactFixtures(page, token)
  const applicationImpact = await request<{ references: Array<{ sourceType: string }> }>(page, token, `/deletion-impact/application/${fixtures.application.id}?pageSize=100`)
  expect(applicationImpact.references.map((reference) => reference.sourceType)).toEqual(expect.arrayContaining(['session', 'invocation']))
  await expectBlocked(page, '/applications', fixtures.application.name)

  const datasetImpact = await request<{ references: Array<{ sourceType: string }> }>(page, token, `/deletion-impact/dataset/${fixtures.dataset.id}?pageSize=100`)
  expect(datasetImpact.references.map((reference) => reference.sourceType)).toContain('evaluation_run')
  await page.goto(`/applications/${fixtures.application.id}?tab=testing&testTab=datasets`)
  await page.getByRole('row').filter({ hasText: fixtures.dataset.name }).getByRole('button', { name: '删除', exact: true }).click()
  const confirmation = page.getByRole('dialog')
  await confirmation.getByRole('button', { name: '删除', exact: true }).click()
  await expect(page.getByText(/已用于评测|referenced|ENTITY_IN_USE|引用/).first()).toBeVisible()

  const workflowImpact = await request<{ references: Array<{ sourceType: string; immutable: boolean }> }>(page, token, `/deletion-impact/workflow/${fixtures.workflow.id}?pageSize=100`)
  expect(workflowImpact.references.map((reference) => reference.sourceType)).toContain('application')
  expect(workflowImpact.references.some((reference) => reference.sourceType === 'evaluation_run' && reference.immutable)).toBe(true)
  await expectBlocked(page, '/workflows', fixtures.workflow.name)
})
