import { fixtureDraftId } from '../draft-fixtures'
import { expect, type APIResponse, type Page, test } from '@playwright/test'
import { createHmac } from 'node:crypto'
import { readFile } from 'node:fs/promises'

const adminPassword = 'agentx-e2e-admin-password'
const gatewayBase = process.env.AGENTX_E2E_RUNTIME_URL ?? ''
const echoBase = process.env.AGENTX_E2E_ECHO_BASE_URL ?? 'http://echo-mcp:8090'

type Execution = {
  id: string
  applicationId?: string | null
  applicationName?: string | null
  workflowId: string
  workflowName: string
  triggerType: string
  triggerName?: string | null
  triggerSourceId?: string | null
  initiatorUserId?: string | null
  initiatorUserName?: string | null
  initiatorDepartmentId?: string | null
  initiatorDepartmentName?: string | null
  status: string
}

type ExecutionPage = {
  items: Execution[]
  limit: number
  total: number
  nextCursor?: string | null
}

type Department = { id: string; parentId?: string | null; name: string; version: number }
type RuntimeContext = { applicationId: string; applicationSlug: string; apiKey: string; workflowId: string; workflowVersionId: string }
type Environment = { id: string; code: string }
type Deployment = { id: string; status: string; publishErrorCode?: string | null; publishErrorMessage?: string | null }
type Webhook = { id: string; name: string; publicId: string; secret: string; version: number }
type Schedule = { id: string; name: string; version: number }
type Credential = { id: string }
type McpServer = { id: string }
type McpTool = { id: string; name: string; title?: string | null }

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill(adminPassword)
  const responsePromise = page.waitForResponse((response) => response.url().endsWith('/api/v1/auth/login') && response.request().method() === 'POST')
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
  return ((await (await responsePromise).json()) as { accessToken: string }).accessToken
}

async function loginViaApi(page: Page) {
  const response = await page.request.post('/api/v1/auth/login', {
    data: { username: 'admin', password: adminPassword },
  })
  const body = await response.text()
  expect(response.ok(), body).toBe(true)
  return (JSON.parse(body) as { accessToken: string }).accessToken
}

async function json<T>(response: APIResponse): Promise<T> {
  const body = await response.text()
  expect(response.ok(), body).toBe(true)
  return JSON.parse(body) as T
}

async function executions(page: Page, headers: Record<string, string>, params: URLSearchParams) {
  return json<ExecutionPage>(await page.request.get(`/api/v1/executions?${params}`, { headers }))
}

function executionParams(values: Record<string, string | undefined | null>) {
  const params = new URLSearchParams({ limit: '100' })
  for (const [key, value] of Object.entries(values)) if (value) params.set(key, value)
  return params
}

async function waitDeployment(page: Page, headers: Record<string, string>, applicationId: string, deploymentId: string) {
  await expect.poll(async () => {
    const values = await json<Deployment[]>(await page.request.get(`/api/v1/applications/${applicationId}/deployments`, { headers }))
    const deployment = values.find((item) => item.id === deploymentId)
    if (deployment?.status === 'rejected') throw new Error(`${deployment.publishErrorCode ?? ''} ${deployment.publishErrorMessage ?? ''}`)
    return deployment?.status
  }, { timeout: 180_000, intervals: [500, 1_000, 2_000] }).toBe('active')
}

test('execution records use authoritative multidimensional filters, Cursor pages, and immutable origin snapshots', async ({ page }) => {
  const accessToken = await login(page)
  let headers = { Authorization: `Bearer ${accessToken}` }
  const contextPath = process.env.AGENTX_V2_08_CONTEXT_OUTPUT
  if (!contextPath) throw new Error('AGENTX_V2_08_CONTEXT_OUTPUT is required')
  const context = JSON.parse(await readFile(contextPath, 'utf8')) as RuntimeContext

  for (let index = 0; index < 9; index += 1) {
    const response = await page.request.post(`${gatewayBase}/gateway/v1/applications/${context.applicationSlug}/invocations`, {
      data: { input: { message: `execution-filter-${index}` }, responseMode: 'async' },
      headers: { Authorization: `Bearer ${context.apiKey}`, 'Idempotency-Key': `execution-filter-${Date.now()}-${index}` },
    })
    expect(response.status(), await response.text()).toBe(202)
  }
  const draft = await json<{ revision: number }>(await page.request.get(`/api/v1/workflows/${context.workflowId}/drafts/${await fixtureDraftId(page, context.workflowId)}`, { headers }))
  const debug = await page.request.post(`/api/v1/workflows/${context.workflowId}/drafts/${await fixtureDraftId(page, context.workflowId)}/debug-executions`, {
    headers,
    data: {
      expectedRevision: draft.revision,
      mode: 'full',
      targetNodeId: null,
      input: { message: 'execution-filter-debug' },
      context: {},
      overlayIds: [],
      sideEffectDecisions: {},
      idempotencyKey: `execution-filter-debug-${Date.now()}`,
    },
  })
  expect(debug.status(), await debug.text()).toBe(202)

  const webhookName = `Execution Filter Webhook ${Date.now().toString(36)}`
  const webhook = await json<Webhook>(await page.request.post(`/api/v1/applications/${context.applicationId}/webhooks`, {
    headers,
    data: { name: webhookName },
  }))
  const scheduleName = `Execution Filter Schedule ${Date.now().toString(36)}`
  const schedule = await json<Schedule>(await page.request.post(`/api/v1/applications/${context.applicationId}/schedules`, {
    headers,
    data: {
      name: scheduleName,
      cronExpression: '*/5 * * * * *',
      timezone: 'Asia/Shanghai',
      input: { message: 'execution-filter-schedule' },
      misfirePolicy: 'fire_once',
    },
  }))
  const environment = (await json<Environment[]>(await page.request.get('/api/v1/environments', { headers }))).find((item) => item.code === 'development')
  expect(environment).toBeTruthy()
  const deployment = await json<Deployment>(await page.request.post(`/api/v1/applications/${context.applicationId}/deployments`, {
    headers,
    data: { workflowVersionId: context.workflowVersionId, environmentId: environment?.id, sessionVersionPolicy: 'pinned' },
  }))
  await waitDeployment(page, headers, context.applicationId, deployment.id)
  const webhookBody = JSON.stringify({ message: 'execution-filter-webhook' })
  const webhookTimestamp = Math.floor(Date.now() / 1_000).toString()
  const webhookSignature = createHmac('sha256', webhook.secret).update(`${webhookTimestamp}.${webhookBody}`).digest('base64url')
  const webhookResponse = await page.request.post(`${gatewayBase}/gateway/v1/webhooks/${webhook.publicId}`, {
    data: webhookBody,
    headers: {
      'Content-Type': 'application/json',
      'Idempotency-Key': `execution-filter-webhook-${Date.now()}`,
      'X-Agentx-Signature': webhookSignature,
      'X-Agentx-Timestamp': webhookTimestamp,
    },
  })
  expect(webhookResponse.status(), await webhookResponse.text()).toBe(202)
  await expect.poll(async () => {
    const values = await executions(page, headers, executionParams({ applicationIds: context.applicationId, statuses: 'succeeded' }))
    return values.items.filter((item) => item.triggerType === 'api_key').length
  }, { timeout: 180_000, intervals: [250, 500, 1_000, 2_000] }).toBeGreaterThanOrEqual(9)
  let webhookExecution: Execution | undefined
  await expect.poll(async () => {
    const values = await executions(page, headers, executionParams({ triggerTypes: 'webhook', triggerName: webhookName, statuses: 'succeeded' }))
    webhookExecution = values.items[0]
    return Boolean(webhookExecution)
  }, { timeout: 180_000, intervals: [250, 500, 1_000, 2_000] }).toBe(true)
  if (!webhookExecution) throw new Error('Webhook Execution was not created')
  expect(webhookExecution.triggerSourceId).toBe(webhook.id)
  expect(webhookExecution.triggerName).toBe(webhookName)
  let scheduleExecution: Execution | undefined
  await expect.poll(async () => {
    const values = await executions(page, headers, executionParams({ triggerTypes: 'schedule', triggerName: scheduleName, statuses: 'succeeded' }))
    scheduleExecution = values.items[0]
    return Boolean(scheduleExecution)
  }, { timeout: 180_000, intervals: [500, 1_000, 2_000] }).toBe(true)
  expect(scheduleExecution?.triggerSourceId).toBe(schedule.id)
  expect(scheduleExecution?.triggerName).toBe(scheduleName)

  const renamedWebhook = `${webhookName} Renamed`
  await json<Webhook>(await page.request.patch(`/api/v1/applications/${context.applicationId}/webhooks/${webhook.id}`, {
    headers,
    data: { name: renamedWebhook, status: 'active', version: webhook.version },
  }))
  const historicalWebhook = await executions(page, headers, executionParams({ triggerTypes: 'webhook', triggerName: webhookName, search: webhookExecution.id }))
  expect(historicalWebhook.items).toHaveLength(1)
  expect(historicalWebhook.items[0].triggerName).toBe(webhookName)

  const cursorFirst = await executions(page, headers, new URLSearchParams({ limit: '8' }))
  expect(cursorFirst.limit).toBe(8)
  const unfiltered = await executions(page, headers, executionParams({}))
  expect(unfiltered.total, 'The scenario must create enough Executions to exercise Cursor pagination').toBeGreaterThan(8)
  expect(cursorFirst.nextCursor).toBeTruthy()

  const applicationExecution = unfiltered.items.find((item) => item.applicationId === context.applicationId && item.status === 'succeeded')
  const namedTriggerExecution = unfiltered.items.find((item) => item.triggerName)
  const initiatedExecution = unfiltered.items.find((item) => item.initiatorUserId && item.initiatorDepartmentId)
  expect(applicationExecution, 'The API-first Application must have a completed Execution').toBeTruthy()
  expect(namedTriggerExecution, 'The API Key trigger must retain its execution-time name').toBeTruthy()
  expect(initiatedExecution, 'A user-triggered Execution must retain its user and department snapshot').toBeTruthy()
  const evaluationExecutions = await executions(page, headers, executionParams({ triggerTypes: 'evaluation' }))
  expect(evaluationExecutions.items.some((item) => item.initiatorUserId && item.initiatorDepartmentId), 'Evaluation must retain its creator snapshot').toBe(true)

  const applicationFiltered = await executions(page, headers, executionParams({ applicationIds: applicationExecution?.applicationId }))
  expect(applicationFiltered.items.length).toBeGreaterThan(0)
  expect(applicationFiltered.items.every((item) => item.applicationId === applicationExecution?.applicationId)).toBe(true)

  const triggerNeedle = namedTriggerExecution?.triggerName?.slice(1, -1).toLocaleUpperCase() || namedTriggerExecution?.triggerName
  const triggerFiltered = await executions(page, headers, executionParams({ triggerName: triggerNeedle }))
  expect(triggerFiltered.items.some((item) => item.id === namedTriggerExecution?.id)).toBe(true)

  const combined = executionParams({
    applicationIds: applicationExecution?.applicationId,
    workflowIds: applicationExecution?.workflowId,
    triggerTypes: applicationExecution?.triggerType,
    triggerName: applicationExecution?.triggerName,
    statuses: applicationExecution?.status,
  })
  const combinedPage = await executions(page, headers, combined)
  expect(combinedPage.items.some((item) => item.id === applicationExecution?.id)).toBe(true)
  expect(combinedPage.items.every((item) => item.applicationId === applicationExecution?.applicationId)).toBe(true)
  expect(combinedPage.items.every((item) => item.workflowId === applicationExecution?.workflowId)).toBe(true)
  expect(combinedPage.items.every((item) => item.triggerType === applicationExecution?.triggerType)).toBe(true)
  expect(combinedPage.items.every((item) => item.status === applicationExecution?.status)).toBe(true)

  const invalidUuid = await page.request.get('/api/v1/executions?workflowIds=not-a-uuid', { headers })
  expect(invalidUuid.status()).toBe(400)
  const tooMany = Array.from({ length: 51 }, (_, index) => `00000000-0000-0000-0000-${index.toString().padStart(12, '0')}`).join(',')
  expect((await page.request.get(`/api/v1/executions?workflowIds=${tooMany}`, { headers })).status()).toBe(400)

  await page.goto('/executions')
  const createdAfter = page.getByRole('button', { name: '开始时间下限' })
  await createdAfter.click()
  await expect(page.getByRole('grid')).toBeVisible()
  await page.getByRole('button', { name: '现在' }).click()
  const timeFilterRequest = page.waitForResponse((response) => response.url().includes('/api/v1/executions?') && new URL(response.url()).searchParams.has('createdAfter'))
  await page.getByRole('button', { name: '确认' }).click()
  expect((await timeFilterRequest).ok()).toBe(true)
  await expect.poll(() => new URL(page.url()).searchParams.has('createdAfter')).toBe(true)
  await createdAfter.click()
  const clearTimeFilterRequest = page.waitForResponse((response) => response.url().includes('/api/v1/executions?') && !new URL(response.url()).searchParams.has('createdAfter'))
  await page.getByRole('button', { name: '清除' }).click()
  expect((await clearTimeFilterRequest).ok()).toBe(true)
  await expect.poll(() => new URL(page.url()).searchParams.has('createdAfter')).toBe(false)

  const firstPageIds = await page.getByRole('row').locator('a[href^="/executions/"]').evaluateAll((links) => links.map((link) => link.getAttribute('href')))
  const nextRequest = page.waitForRequest((request) => request.url().includes('/api/v1/executions?') && new URL(request.url()).searchParams.has('cursor'))
  await page.getByRole('button', { name: '下一页' }).click()
  await nextRequest
  expect(new URL(page.url()).searchParams.has('cursor'), 'Cursor is table state and must not leak into the URL').toBe(false)
  await expect(page.getByRole('button', { name: '上一页' })).toBeEnabled()
  const secondPageIds = await page.getByRole('row').locator('a[href^="/executions/"]').evaluateAll((links) => links.map((link) => link.getAttribute('href')))
  expect(secondPageIds).not.toEqual(firstPageIds)

  const searched = page.waitForResponse((response) => response.url().includes('/api/v1/executions?') && new URL(response.url()).searchParams.get('search') === applicationExecution?.id)
  await page.getByPlaceholder('搜索执行 ID、Trace ID 或错误码').fill(applicationExecution?.id ?? '')
  await searched
  await expect(page.getByText(applicationExecution?.id ?? '', { exact: true })).toBeVisible()
  expect(new URL(page.url()).searchParams.get('search')).toBe(applicationExecution?.id)

  await page.getByPlaceholder('搜索执行 ID、Trace ID 或错误码').fill('')
  await expect.poll(() => new URL(page.url()).searchParams.has('search')).toBe(false)
  await page.getByRole('button', { name: '更多筛选' }).click()
  await page.getByRole('button', { name: '选择应用' }).click()
  await page.getByRole('textbox', { name: '搜索' }).fill(applicationExecution?.applicationName ?? '')
  await page.getByRole('option', { name: applicationExecution?.applicationName ?? '' }).click()
  await expect.poll(() => new URL(page.url()).searchParams.get('applicationIds')).toBe(applicationExecution?.applicationId)
  await expect(page.getByRole('row').filter({ hasText: applicationExecution?.workflowName ?? '' }).first()).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('button', { name: '选择应用' })).toHaveAttribute('aria-expanded', 'false')

  const departmentsForTool = await json<Department[]>(await page.request.get('/api/v1/departments', { headers }))
  const rootDepartment = departmentsForTool.find((item) => !item.parentId) ?? departmentsForTool[0]
  if (!rootDepartment) throw new Error('A root Department is required')
  const credential = await json<Credential>(await page.request.post('/api/v1/credentials', {
    headers,
    data: { name: `Execution Filter Credential ${Date.now()}`, credentialType: 'bearer', secret: 'execution-filter-secret', ownerDepartmentId: rootDepartment.id },
  }))
  const server = await json<McpServer>(await page.request.post('/api/v1/mcp/servers', {
    headers,
    data: {
      name: `Execution Filter MCP ${Date.now()}`,
      description: 'Execution filter option fixture',
      ownerDepartmentId: rootDepartment.id,
      transport: {
        kind: 'streamable_http',
        endpoint: `${echoBase}/mcp`,
        bearerCredentialId: credential.id,
      },
      configuration: {},
    },
  }))
  const discovery = await json<{ tools: McpTool[] }>(await page.request.post(`/api/v1/mcp/servers/${server.id}/discover`, { headers, data: {} }))
  const tool = discovery.tools.find((item) => item.name === 'echo') ?? discovery.tools[0]
  if (!tool) throw new Error('MCP discovery returned no tools')
  await page.getByRole('button', { name: '选择工具' }).click()
  await page.getByRole('textbox', { name: '搜索' }).fill(tool.title ?? tool.name)
  const toolResponse = page.waitForResponse((response) => response.url().includes('/api/v1/executions?') && new URL(response.url()).searchParams.get('toolIds') === tool.id)
  await page.getByRole('option', { name: new RegExp(tool.title ?? tool.name) }).click()
  expect((await toolResponse).ok()).toBe(true)
  await expect.poll(() => new URL(page.url()).searchParams.get('toolIds')).toBe(tool.id)
  const uncalledTool = await executions(page, headers, executionParams({ applicationIds: context.applicationId, toolIds: tool.id }))
  expect(uncalledTool.total, 'A discoverable but uncalled MCP Tool must not match').toBe(0)

  const origin = initiatedExecution as Execution
  const departments = await json<Department[]>(await page.request.get('/api/v1/departments', { headers }))
  const department = departments.find((item) => item.id === origin.initiatorDepartmentId)
  expect(department).toBeTruthy()
  const childDepartment = await json<Department>(await page.request.post('/api/v1/departments', {
    headers,
    data: { name: `Execution Filter Child ${Date.now().toString(36)}`, parentId: department?.id },
  }))
  const childDepartmentResult = await executions(page, headers, executionParams({
    initiatorDepartmentIds: childDepartment.id,
    search: origin.id,
  }))
  expect(childDepartmentResult.total, 'Department filtering must not include child departments').toBe(0)
  const renamed = `${department?.name} Snapshot ${Date.now().toString(36)}`
  let updated: Department | undefined
  try {
    updated = await json<Department>(await page.request.patch(`/api/v1/departments/${department?.id}`, {
      headers,
      data: { name: renamed, parentId: department?.parentId ?? null, version: department?.version },
    }))
    // Updating a department advances the token version for affected users.
    // Continue the assertions with a fresh token instead of using the
    // deliberately revoked scenario token.
    const refreshedToken = await login(page)
    headers = { Authorization: `Bearer ${refreshedToken}` }
    const historicalParams = executionParams({
      initiatorDepartmentIds: department?.id,
      search: origin.id,
    })
    // Department updates are projected to Runtime asynchronously. Wait for
    // the new token version to be accepted before asserting filtered results.
    await expect.poll(
      async () => (await page.request.get(`/api/v1/executions?${historicalParams}`, { headers })).status(),
      { timeout: 30_000, intervals: [250, 500, 1_000, 2_000] },
    ).toBe(200)
    const historical = await executions(page, headers, historicalParams)
    expect(historical.items).toHaveLength(1)
    expect(historical.items[0].initiatorDepartmentName).toBe(origin.initiatorDepartmentName)
    expect(historical.items[0].initiatorDepartmentName).not.toBe(renamed)

    await page.goto(`/executions?initiatorDepartmentIds=${department?.id}&search=${origin.id}`)
    const row = page.getByRole('row').filter({ hasText: origin.id })
    await expect(row).toContainText(origin.initiatorDepartmentName ?? '')
    await expect(row).not.toContainText(renamed)
  } finally {
    if (updated && department) {
      // Department changes revoke affected users' access tokens by design.
      // Re-authenticate before restoring the fixture so cleanup cannot fail
      // with SESSION_REVOKED after the scenario has already passed.
      const cleanupToken = await loginViaApi(page)
      await json<Department>(await page.request.patch(`/api/v1/departments/${department.id}`, {
        headers: { Authorization: `Bearer ${cleanupToken}` },
        data: { name: department.name, parentId: department.parentId ?? null, version: updated.version },
      }))
    }
  }
})
