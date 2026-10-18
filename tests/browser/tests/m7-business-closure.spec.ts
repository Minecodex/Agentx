import { beginApplicationEvaluation } from '../support/application-testing'
import { publishFixtureVersion } from '../draft-fixtures'
import { fixtureDraftId } from '../draft-fixtures'
import { createHmac } from 'node:crypto'

import { expect, type APIResponse, type Page, test } from '@playwright/test'

import { publishChatMapping, useRuntimePortForward } from './playground-helpers'

const password = 'agentx-e2e-admin-password'
const gatewayBase = process.env.AGENTX_E2E_RUNTIME_URL ?? ''

type PageResponse<T> = { items: T[] }
type Workflow = { id: string; name: string; latestVersion?: number; version: number; description?: string; visibility: string; serviceIdentityId?: string }
type ResourceReference = { resourceType: string; resourceId: string; resourceVersionId?: string; operation: string }
type WorkflowDefinition = {
  schemaVersion?: string
  start?: { inputs?: { properties?: Record<string, unknown> } }
  nodes?: Array<{ type?: string; resourceReferences?: ResourceReference[] }>
}
type WorkflowVersion = { id: string; versionNumber: number; schemaVersion?: string; definition?: WorkflowDefinition }
type Grant = { id: string; subjectType: string; subjectId: string; resourceVersionId?: string; operation: string }
type Environment = { id: string; code: string }
type Application = { id: string; name: string; slug: string; status: string; description?: string; visibility: string; version: number }
type ApplicationDeployment = { id: string; status: string; publishErrorCode?: string | null; publishErrorMessage?: string | null }
type Invocation = { id: string; executionId?: string; status: string; outputs?: Record<string, unknown> }
type Execution = { id: string; status: string; error?: unknown }
type Approval = { id: string; executionId: string; workflowId: string; status: string }
type Webhook = { publicId: string; secret: string }
type RetentionRun = { id: string; status: string; dryRun: boolean }
type Schedule = { id: string; name: string; cronExpression: string; timezone: string; input: unknown; misfirePolicy: string; status: string; version: number }
type ExecutionPage = { items: Array<{ id: string; workflowName: string; triggerType: string; triggerName?: string | null; status: string }>; total: number }

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill(password)
  const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/auth/login') && value.request().method() === 'POST')
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
  return ((await (await response).json()) as { accessToken: string }).accessToken
}

async function request<T>(page: Page, token: string, path: string, method = 'GET', body?: unknown, gateway = false): Promise<T> {
  const response = await page.request.fetch(`${gateway ? `${gatewayBase}/gateway/v1` : '/api/v1'}${path}`, {
    method,
    data: body,
    headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() },
  })
  await expectResponse(response, path)
  return response.body().then((value) => value.length ? JSON.parse(value.toString()) as T : undefined as T)
}

async function expectResponse(response: APIResponse, label: string) {
  if (!response.ok()) throw new Error(`${label}: ${response.status()} ${await response.text()}`)
}

async function waitInvocation(page: Page, token: string, id: string, terminal = false) {
  let invocation: Invocation | undefined
  await expect.poll(async () => {
    invocation = await request<Invocation>(page, token, `/invocations/${id}`, 'GET', undefined, true)
    return terminal ? ['completed', 'failed', 'cancelled'].includes(invocation.status) : invocation.executionId
  }, { timeout: 180_000, intervals: [500, 750, 1_000, 2_000] }).toBeTruthy()
  return invocation!
}

async function approveExecution(page: Page, token: string, executionId: string) {
  let approval: Approval | undefined
  await expect.poll(async () => {
    const result = await request<PageResponse<Approval>>(page, token, '/approvals?pageSize=100')
    approval = result.items.find((item) => item.executionId === executionId && item.status === 'pending')
    if (!approval) {
      const execution = await request<Execution>(page, token, `/executions/${executionId}`)
      if (['failed', 'cancelled', 'timed_out'].includes(execution.status)) throw new Error(`Execution stopped before Approval: ${JSON.stringify(execution)}`)
    }
    return approval?.id
  }, { timeout: 150_000, intervals: [500, 1_000, 2_000] }).toBeTruthy()
  await approveApproval(page, token, approval!)
  return approval!
}

async function approveApproval(page: Page, token: string, approval: Approval) {
  await page.goto(`/approvals/${approval.id}`)
  await page.getByRole('button', { name: '领取' }).click()
  await page.getByRole('button', { name: '通过' }).click()
  await page.getByRole('dialog', { name: '通过' }).getByRole('button', { name: '通过' }).click()
  await expect.poll(async () => {
    const values = await request<PageResponse<Approval>>(page, token, '/approvals?pageSize=100')
    return values.items.find((item) => item.id === approval.id)?.status
  }).toBe('decided')
}

async function approveInvocation(page: Page, approvalPage: Page, token: string, invocationId: string) {
  const started = await waitInvocation(page, token, invocationId)
  await approveExecution(approvalPage, token, started.executionId!)
  const completed = await waitInvocation(page, token, invocationId, true)
  expect(completed.status).toBe('completed')
  return completed
}

async function waitApplicationDeployment(page: Page, token: string, applicationId: string, deploymentId: string) {
  await expect.poll(async () => {
    const deployments = await request<ApplicationDeployment[]>(page, token, `/applications/${applicationId}/deployments`)
    const deployment = deployments.find((item) => item.id === deploymentId)
    if (deployment?.status === 'failed') {
      throw new Error(`Application Deployment failed: ${deployment.publishErrorCode ?? 'UNKNOWN'} ${deployment.publishErrorMessage ?? ''}`)
    }
    return deployment?.status
  }, { timeout: 120_000, intervals: [500, 1_000, 2_000] }).toBe('active')
}

async function setLocale(page: Page, locale: 'zh-CN' | 'en-US') {
  if (await page.locator('html').getAttribute('lang') === locale) return
  await page.getByRole('button', { name: /^(语言|Language)$/ }).click()
  await page.getByRole('menuitemradio', { name: locale === 'zh-CN' ? '简体中文' : 'English' }).click()
  await expect(page.locator('html')).toHaveAttribute('lang', locale)
}

async function setTheme(page: Page, theme: 'light' | 'dark') {
  await page.getByRole('button', { name: /^(主题|Theme)$/ }).click()
  await page.getByRole('menuitemradio', { name: theme === 'light' ? /^(浅色|Light)$/ : /^(深色|Dark)$/ }).click()
  await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
}

async function findM6Workflow(page: Page, token: string) {
  const workflows = await request<PageResponse<Workflow>>(page, token, '/workflows?pageSize=100&search=M6%20Studio')
  const candidates = workflows.items.filter((item) => item.name.startsWith('M6 Studio'))
  for (const candidate of candidates) {
    const summaries = await request<WorkflowVersion[]>(page, token, `/workflows/${candidate.id}/versions`)
    const versions = await Promise.all(summaries.filter(item=>item.versionNumber===1).map(item=>request<WorkflowVersion>(page,token,`/workflows/${candidate.id}/versions/${item.id}`)))
    const version = versions.find((item) => {
      const definition = item.definition
      const properties = definition?.start?.inputs?.properties ?? {}
      const nodes = definition?.nodes ?? []
      const hasChatInputs = 'question' in properties && 'attachments' in properties
      const hasApproval = nodes.some((node) => node.type === 'approval')
      const hasModel = nodes.some((node) => node.resourceReferences?.some((reference) => reference.resourceType === 'model'))
      return item.versionNumber === 1 && (item.schemaVersion ?? definition?.schemaVersion) === '8.0' && hasChatInputs && hasApproval && hasModel
    })
    if (version) return { workflow: candidate, version }
  }
  return { workflow: undefined, version: undefined }
}

async function verifyMissingDefaultMappingIsRejected(page: Page, token: string, environment: Environment, suffix: number) {
  const workflow = await request<Workflow>(page, token, '/workflows', 'POST', {
    name: `M7 Missing Default ${suffix}`,
    description: 'Playground mapping validation fixture',
    visibility: 'company',
  })
  const draft = await request<{ revision: number; editorDocument: Record<string, unknown> }>(page, token, `/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`)
  const saved = await request<{ revision: number }>(page, token, `/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`, 'PUT', {
    expectedRevision: draft.revision,
    definition: {
      schemaVersion: '8.0',
      start: {
        inputs: {
          type: 'object',
          properties: { question: { type: 'string' }, tenant: { type: 'string' } },
          required: ['question', 'tenant'],
          additionalProperties: false,
        },
        contexts: {},
      },
      nodes: [
        { id: 'echo-input', key: 'echo_input', type: 'set', typeVersion: 1, name: 'Echo Input', disabled: false, parameters: { values: { kind: 'object', fields: {} }, keepOnlySet: false }, contextWrites: [], resourceReferences: [], settings: {} },
        {
          id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true,
          parameters: {
            outputs: { answer: { kind: 'reference', selector: { namespace: 'inputs', path: ['question'] }, missingPolicy: { kind: 'error' } } },
            errorOutputs: {},
          },
          contextWrites: [], resourceReferences: [], settings: {},
        },
      ],
      connections: [
        { id: 'start-echo', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'echo-input', targetHandle: 'main', order: 0 },
        { id: 'echo-end', sourceNodeId: 'echo-input', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 },
      ],
      end: {
        outputs: {
          answer: { schema: { type: 'string' }, required: true, sensitive: false },
        },
      },
      settings: { executionOrder: 'deterministic' },
    },
    editorDocument: { ...draft.editorDocument, nodeLayouts: [{ nodeId: 'echo-input', x: 220, y: 160 }], edges: [{ edgeId: 'start-echo' }, { edgeId: 'echo-end' }] },
  })
  const version = await publishFixtureVersion<WorkflowVersion>(page, token, workflow.id, saved.revision)
  await request(page, token, `/workflows/${workflow.id}/deployments`, 'POST', {contentSource:'version', workflowVersionId: version.id, environmentId: environment.id })
  const application = await request<Application>(page, token, '/applications', 'POST', {
    workflowId: workflow.id,
    name: `M7 Missing Default ${suffix}`,
    slug: `m7-missing-default-${suffix}`,
    description: 'Playground mapping validation fixture',
    visibility: 'company',
  })
  const deployment = await request<ApplicationDeployment>(page, token, `/applications/${application.id}/deployments`, 'POST', {
    workflowVersionId: version.id,
    environmentId: environment.id,
    sessionVersionPolicy: 'pinned',
  })
  await waitApplicationDeployment(page, token, application.id, deployment.id)
  const response = await page.request.put(`/api/v1/applications/${application.id}/deployments/${deployment.id}/playground-config`, {
    headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() },
    data: { expectedVersion: 0, mapping: { questionInput: 'question', fileInput: null, answerOutput: 'answer', answerFilesOutput: null } },
  })
  expect(response.status()).toBe(202)
  expect(await response.json()).toMatchObject({ publishStatus: 'publishing' })
}

async function createFileEchoPlaygroundFixture(page: Page, token: string, environment: Environment, suffix: number) {
  const name = `M7 File Echo ${suffix}`
  const workflow = await request<Workflow>(page, token, '/workflows', 'POST', {
    name,
    description: 'Playground Assistant file output fixture',
    visibility: 'company',
  })
  const draft = await request<{ revision: number; editorDocument: Record<string, unknown> }>(page, token, `/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`)
  const saved = await request<{ revision: number }>(page, token, `/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`, 'PUT', {
    expectedRevision: draft.revision,
    definition: {
      schemaVersion: '8.0',
      start: {
        inputs: {
          type: 'object',
          properties: {
            question: { type: 'string', title: 'Question' },
            files: {
              type: 'array',
              title: 'Files',
              minItems: 0,
              maxItems: 3,
              'x-agentx-artifact': true,
              'x-agentx-artifact-array': true,
              'x-agentx-content-types': ['image/*'],
              'x-agentx-max-size-bytes': 1048576,
              'x-agentx-max-total-size-bytes': 2097152,
            },
          },
          required: ['question'],
          additionalProperties: false,
        },
        contexts: {},
      },
      nodes: [
        { id: 'echo-input', key: 'echo_input', type: 'set', typeVersion: 1, name: 'Echo Input', disabled: false, parameters: { values: { kind: 'object', fields: {} }, keepOnlySet: false }, contextWrites: [], resourceReferences: [], settings: {} },
        { id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true, parameters: {
          outputs: {
            answer: { kind: 'reference', selector: { namespace: 'inputs', path: ['question'] }, missingPolicy: { kind: 'error' } },
            answer_files: { kind: 'reference', selector: { namespace: 'inputs', path: ['files'] }, missingPolicy: { kind: 'error' } },
          },
          errorOutputs: {},
        }, contextWrites: [], resourceReferences: [], settings: {} },
      ],
      connections: [
        { id: 'start-echo', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'echo-input', targetHandle: 'main', order: 0 },
        { id: 'echo-end', sourceNodeId: 'echo-input', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 },
      ],
      end: {
        outputs: {
          answer: { schema: { type: 'string' }, required: true, sensitive: false },
          answer_files: { schema: { type: 'array', 'x-agentx-artifact': true, 'x-agentx-artifact-array': true }, required: true, sensitive: false },
        },
      },
      settings: { executionOrder: 'deterministic' },
    },
    editorDocument: { ...draft.editorDocument, nodeLayouts: [{ nodeId: 'echo-input', x: 220, y: 160 }], edges: [{ edgeId: 'start-echo' }, { edgeId: 'echo-end' }] },
  })
  const version = await publishFixtureVersion<WorkflowVersion>(page, token, workflow.id, saved.revision)
  await request(page, token, `/workflows/${workflow.id}/deployments`, 'POST', {contentSource:'version', workflowVersionId: version.id, environmentId: environment.id })
  const application = await request<Application>(page, token, '/applications', 'POST', {
    workflowId: workflow.id,
    name,
    slug: `m7-file-echo-${suffix}`,
    description: 'Playground Assistant file output fixture',
    visibility: 'company',
  })
  const deployment = await request<ApplicationDeployment>(page, token, `/applications/${application.id}/deployments`, 'POST', {
    workflowVersionId: version.id,
    environmentId: environment.id,
    sessionVersionPolicy: 'pinned',
  })
  await waitApplicationDeployment(page, token, application.id, deployment.id)
  return { application, deployment }
}

async function verifyFileEchoPlayground(page: Page, token: string, application: Application) {
  await page.goto('/playground')
  await page.getByRole('combobox').click()
  await page.getByRole('option', { name: application.name, exact: true }).click()
  await page.getByRole('tab', { name: '对话测试' }).click()
  await page.getByRole('button', { name: '新建会话' }).click()
  await expect(page.getByPlaceholder('请先配置对话参数映射')).toBeVisible()
  await page.getByRole('button', { name: '参数映射' }).click()
  const mappingDialog = page.getByRole('dialog', { name: '对话参数映射' })
  await mappingDialog.getByRole('combobox', { name: '文件输入' }).click()
  await page.getByRole('option', { name: /files.*artifact/i }).click()
  await mappingDialog.getByRole('combobox', { name: '回答文件输出' }).click()
  await page.getByRole('option', { name: /answer_files.*artifact/i }).click()
  await mappingDialog.getByRole('button', { name: '保存并发布' }).click()
  await expect(mappingDialog).toBeHidden()
  await expect(page.getByText(/映射 v\d+/)).toBeVisible({ timeout: 120_000 })

  await page.locator('input[type=file]').setInputFiles({ name: 'm7-echo-image.png', mimeType: 'image/png', buffer: Buffer.from('M7 Assistant file echo') })
  const invocationResponse = page.waitForResponse((value) => value.url().includes('/gateway/v1/sessions/') && value.url().endsWith('/messages') && value.request().method() === 'POST')
  await page.getByPlaceholder('输入消息进行测试…').fill('M7 file echo')
  await page.getByRole('button', { name: '发送' }).click()
  const invocationHttp = await invocationResponse
  expect(invocationHttp.status()).toBe(202)
  const completed = await waitInvocation(page, token, (await invocationHttp.json() as Invocation).id, true)
  expect(completed.status).toBe('completed')
  const echoMessages = page.getByRole('article').filter({ hasText: 'M7 file echo' })
  await expect(echoMessages).toHaveCount(2, { timeout: 30_000 })
  await expect(echoMessages.getByText('M7 file echo', { exact: true })).toHaveCount(2)
  await expect(page.getByRole('link', { name: '执行详情 / Trace' })).toHaveAttribute('href', `/executions/${completed.executionId}`)
  await expect(page.getByRole('button', { name: /m7-echo-image\.png/ })).toHaveCount(2)
}

test('M7 closes Application, Trigger, Evaluation, Approval and governance paths on the shared Runtime', async ({ context, page }, testInfo) => {
  await useRuntimePortForward(page)
  const token = await login(page)
  const suffix = Date.now()
  const { workflow, version } = await findM6Workflow(page, token)
  expect(workflow, 'M6 Studio must publish the Workflow used by M7').toBeTruthy()
  expect(version, 'M6 Studio must publish a Definition 8.0 chat-capable v1').toBeTruthy()
  const environments = await request<Environment[]>(page, token, '/environments')
  const environment = environments.find((item) => item.code === 'development')
  expect(environment).toBeTruthy()
  await verifyMissingDefaultMappingIsRejected(page, token, environment!, suffix)
  const fileEcho = await createFileEchoPlaygroundFixture(page, token, environment!, suffix)
  await verifyFileEchoPlayground(page, token, fileEcho.application)

  const triggerName = `M7 Remote Trigger ${suffix}`
  const triggerWorkflow = await request<Workflow>(page, token, '/workflows', 'POST', {
    name: triggerName,
    description: 'M7 Poll and Lifecycle integration',
    visibility: 'company',
  })
  const triggerDraft = await request<{ revision: number; editorDocument: Record<string, unknown> }>(page, token, `/workflows/${triggerWorkflow.id}/drafts/${await fixtureDraftId(page, triggerWorkflow.id)}`)
  const savedTriggerDraft = await request<{ revision: number }>(page, token, `/workflows/${triggerWorkflow.id}/drafts/${await fixtureDraftId(page, triggerWorkflow.id)}`, 'PUT', {
    expectedRevision: triggerDraft.revision,
    definition: {
      schemaVersion: '8.0',
      start: {
        inputs: { type: 'object', properties: { source: { type: 'string' } }, additionalProperties: true },
        contexts: {},
      },
      nodes: [
        { id: 'trigger-set', key: 'trigger_set', type: 'set', typeVersion: 1, name: 'Trigger Set', disabled: false, parameters: { values: { kind: 'object', fields: { triggered: { kind: 'literal', value: true } } }, keepOnlySet: false }, contextWrites: [], resourceReferences: [], settings: {} },
        { id: 'set-result', key: 'set_result', type: 'set', typeVersion: 1, name: 'Set Result', disabled: false, parameters: { values: { kind: 'object', fields: {} }, keepOnlySet: false }, contextWrites: [], resourceReferences: [], settings: {} },
        {
          id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true,
          parameters: {
            outputs: { result: { kind: 'reference', selector: { namespace: 'outputs', sourceNodeId: 'set-result', port: 'main', run: { kind: 'current' }, item: { kind: 'current' }, path: [] }, missingPolicy: { kind: 'error' } } },
            errorOutputs: {},
          },
          contextWrites: [], resourceReferences: [], settings: {},
        },
      ],
      connections: [
        { id: 'start-to-set', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'trigger-set', targetHandle: 'main', order: 0 },
        { id: 'set-to-set', sourceNodeId: 'trigger-set', sourceHandle: 'main', targetNodeId: 'set-result', targetHandle: 'main', order: 0 },
        { id: 'set-to-end', sourceNodeId: 'set-result', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 },
      ],
      end: {
        outputs: {
          result: { schema: { type: 'object' }, required: true, sensitive: false },
        },
      },
      settings: { executionOrder: 'deterministic' },
    },
    editorDocument: {
      ...triggerDraft.editorDocument,
      nodeLayouts: [{ nodeId: 'trigger-set', x: 80, y: 160 }, { nodeId: 'set-result', x: 360, y: 160 }],
      edges: [{ edgeId: 'set-to-set' }],
    },
  })
  const triggerVersion = await publishFixtureVersion<WorkflowVersion>(page, token, triggerWorkflow.id, savedTriggerDraft.revision)
  await request(page, token, `/workflows/${triggerWorkflow.id}/deployments`, 'POST', {contentSource:'version', workflowVersionId: triggerVersion.id, environmentId: environment!.id })
  const triggerApplication = await request<Application>(page, token, '/applications', 'POST', {
    workflowId: triggerWorkflow.id,
    name: triggerName,
    slug: `m7-remote-trigger-${suffix}`,
    description: 'M7 Poll and Lifecycle Application',
    visibility: 'company',
  })
  const initialTriggerDeployment = await request<ApplicationDeployment>(page, token, `/applications/${triggerApplication.id}/deployments`, 'POST', {
    workflowVersionId: triggerVersion.id,
    environmentId: environment!.id,
    sessionVersionPolicy: 'pinned',
  })
  await waitApplicationDeployment(page, token, triggerApplication.id, initialTriggerDeployment.id)
  const schedule = await request<Schedule>(page, token, `/applications/${triggerApplication.id}/schedules`, 'POST', {
    name: 'M7 Fire Once',
    cronExpression: '*/5 * * * * *',
    timezone: 'Asia/Shanghai',
    input: { source: 'm7-schedule' },
    misfirePolicy: 'fire_once',
  })
  expect(schedule.misfirePolicy).toBe('fire_once')
  await new Promise((resolve) => setTimeout(resolve, 6_000))
  const triggerExecutionPath = `/executions?limit=100&applicationIds=${triggerApplication.id}`
  const draftTriggerExecutions = await request<ExecutionPage>(page, token, triggerExecutionPath)
  expect(draftTriggerExecutions.items.filter((item) => item.triggerType === 'schedule'), 'Schedule drafts must not activate before the next Application Deployment').toHaveLength(0)
  const publishedTriggerDeployment = await request<ApplicationDeployment>(page, token, `/applications/${triggerApplication.id}/deployments`, 'POST', {
    workflowVersionId: triggerVersion.id,
    environmentId: environment!.id,
    sessionVersionPolicy: 'pinned',
  })
  await waitApplicationDeployment(page, token, triggerApplication.id, publishedTriggerDeployment.id)
  await expect.poll(async () => (await request<ExecutionPage>(page, token, triggerExecutionPath)).items.filter((item) => item.triggerType === 'schedule' && item.status === 'succeeded').length, { timeout: 120_000, intervals: [500, 1_000, 2_000] }).toBeGreaterThanOrEqual(2)
  const currentTriggerApplication = await request<Application>(page, token, `/applications/${triggerApplication.id}`)
  const disabledTriggerApplication = await request<Application>(page, token, `/applications/${triggerApplication.id}`, 'PATCH', {
    name: currentTriggerApplication.name,
    description: currentTriggerApplication.description ?? null,
    visibility: currentTriggerApplication.visibility,
    status: 'disabled',
    version: currentTriggerApplication.version,
  })
  expect(disabledTriggerApplication.status).toBe('disabled')
  await expect.poll(async () => {
    const response = await page.request.post(`${gatewayBase}/gateway/v1/applications/${triggerApplication.slug}/invocations`, {
      data: { input: { source: 'disabled-application' } },
      headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': `m7-disabled-${suffix}` },
    })
    return response.status()
  }, { timeout: 30_000, intervals: [500, 1_000, 2_000] }).toBe(401)
  const triggerExecutions = await request<ExecutionPage>(page, token, triggerExecutionPath)
  const scheduleExecutions = triggerExecutions.items.filter((item) => item.triggerType === 'schedule')
  expect(scheduleExecutions.length).toBeGreaterThanOrEqual(2)
  expect(scheduleExecutions.every((item) => item.status === 'succeeded')).toBeTruthy()
  expect(scheduleExecutions.every((item) => item.triggerName === schedule.name)).toBeTruthy()

  const application = await request<Application>(page, token, '/applications', 'POST', {
    workflowId: workflow!.id,
    name: `M7 Closure ${suffix}`,
    slug: `m7-closure-${suffix}`,
    description: 'M7 Kubernetes business closure',
    visibility: 'company',
  })
  const applicationDeployment = await request<ApplicationDeployment>(page, token, `/applications/${application.id}/deployments`, 'POST', {
    workflowVersionId: version!.id,
    environmentId: environment!.id,
    sessionVersionPolicy: 'pinned',
  })
  await waitApplicationDeployment(page, token, application.id, applicationDeployment.id)
  const playgroundMapping = {
    questionInput: 'question',
    fileInput: 'attachments',
    answerOutput: 'answer',
    answerFilesOutput: null,
  }

  await page.goto('/playground')
  await page.getByRole('combobox').click()
  await page.getByRole('option', { name: application.name, exact: true }).click()
  await page.getByRole('tab', { name: '对话测试' }).click()
  const sessionResponse = page.waitForResponse((value) => value.url().includes(`/gateway/v1/applications/${application.slug}/sessions`) && value.request().method() === 'POST')
  await page.getByRole('button', { name: '新建会话' }).click()
  const session = await (await sessionResponse).json() as { id: string }
  await expect(page.getByText(session.id, { exact: true })).toBeVisible()
  await expect(page.getByPlaceholder('请先配置对话参数映射')).toBeVisible()
  await page.getByRole('button', { name: '参数映射' }).click()
  const mappingDialog = page.getByRole('dialog', { name: '对话参数映射' })
  await mappingDialog.getByRole('combobox', { name: '文件输入' }).click()
  await page.getByRole('option', { name: /attachments.*artifact/i }).click()
  await mappingDialog.getByRole('button', { name: '保存并发布' }).click()
  await expect(mappingDialog).toBeHidden()
  await expect(page.getByText(/映射 v\d+/)).toBeVisible({ timeout: 120_000 })

  await page.route('**/gateway/v1/artifacts', async (route) => route.abort('connectionfailed'))
  await page.locator('input[type=file]').setInputFiles({ name: 'failed-upload.png', mimeType: 'image/png', buffer: Buffer.from('failed upload') })
  await expect(page.getByRole('region', { name: 'Notifications (F8)' }).getByText('无法连接服务。请检查网络、服务地址或浏览器的安全提示后重试。', { exact: true })).toBeVisible()
  await page.unroute('**/gateway/v1/artifacts')

  await page.route('**/gateway/v1/sessions/*/messages', async (route) => {
    if (route.request().method() !== 'POST') return route.continue()
    await route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ code: 'RUNTIME_UNAVAILABLE', message: 'Runtime unavailable', requestId: `m7-runtime-${suffix}` }) })
  })
  await page.getByPlaceholder('输入消息进行测试…').fill('M7 Runtime unavailable')
  await page.getByRole('button', { name: '发送' }).click()
  await expect(page.getByRole('region', { name: 'Notifications (F8)' }).getByText(/请求暂时无法完成/).first()).toBeVisible()
  await page.unroute('**/gateway/v1/sessions/*/messages')

  await page.locator('input[type=file]').setInputFiles({ name: 'm7-chat-image.png', mimeType: 'image/png', buffer: Buffer.from('M7 chat file input') })
  await expect(page.getByText('m7-chat-image.png')).toBeVisible()
  let interrupted = false
  await page.route('**/gateway/v1/invocations/*/events', async (route) => {
    if (!interrupted) {
      interrupted = true
      await route.abort('connectionfailed')
    } else await route.continue()
  })
  const invocationResponse = page.waitForResponse((value) => value.url().includes('/gateway/v1/sessions/') && value.url().endsWith('/messages') && value.request().method() === 'POST')
  await page.getByPlaceholder('输入消息进行测试…').fill('M7 Playground execution')
  await page.getByRole('button', { name: '发送' }).click()
  const invocationHttp = await invocationResponse
  expect(invocationHttp.status()).toBe(202)
  const invocation = await invocationHttp.json() as Invocation
  const approvalPage = await context.newPage()
  const completed = await approveInvocation(page, approvalPage, token, invocation.id)
  await expect(page.getByText('已完成', { exact: true })).toBeVisible({ timeout: 30_000 })
  await expect.poll(async () => (await request<Array<{ role: string }>>(page, token, `/sessions/${session.id}/messages`, 'GET', undefined, true)).some((message) => message.role === 'assistant'), { timeout: 30_000 }).toBeTruthy()
  await expect(page.getByRole('link', { name: '执行详情 / Trace' })).toHaveAttribute('href', `/executions/${completed.executionId}`)
  await expect(page.getByRole('button', { name: /m7-chat-image\.png/ })).toHaveCount(1)
  await publishChatMapping(page, token, application.id, applicationDeployment.id, playgroundMapping)
  const nextInvocationResponse = page.waitForResponse((value) => value.url().includes('/gateway/v1/sessions/') && value.url().endsWith('/messages') && value.request().method() === 'POST')
  await page.getByPlaceholder('输入消息进行测试…').fill('M7 mapping revision on current session')
  await page.getByRole('button', { name: '发送' }).click()
  const nextInvocationHttp = await nextInvocationResponse
  expect(nextInvocationHttp.status()).toBe(202)
  const nextInvocation = await nextInvocationHttp.json() as Invocation
  await approveInvocation(page, approvalPage, token, nextInvocation.id)
  await expect(page.getByText('M7 mapping revision on current session', { exact: true })).toBeVisible({ timeout: 30_000 })
  await page.reload()
  await expect(page.getByText('M7 mapping revision on current session', { exact: true })).toBeVisible({ timeout: 30_000 })
  await publishChatMapping(page, token, application.id, applicationDeployment.id, null)
  await page.reload()
  await expect(page.getByPlaceholder('请先配置对话参数映射')).toBeVisible({ timeout: 30_000 })
  await page.unroute('**/gateway/v1/invocations/*/events')

  const idempotencyKey = `m7-idempotency-${suffix}`
  const invoke = () => page.request.post(`${gatewayBase}/gateway/v1/applications/${application.slug}/invocations`, {
    data: { input: { question: 'idempotency-e2e', attachments: [] } },
    headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': idempotencyKey },
  })
  const first = await invoke()
  const second = await invoke()
  await expectResponse(first, 'first idempotent invocation')
  await expectResponse(second, 'replayed idempotent invocation')
  const firstInvocation = await first.json() as Invocation
  const secondInvocation = await second.json() as Invocation
  expect(secondInvocation.id).toBe(firstInvocation.id)
  const referenceInvocation = await approveInvocation(page, approvalPage, token, firstInvocation.id)
  expect(referenceInvocation.outputs).toBeTruthy()

  const webhook = await request<Webhook>(page, token, `/applications/${application.id}/webhooks`, 'POST', { name: 'M7 Signed Webhook' })
  const body = JSON.stringify({ question: 'webhook-e2e', attachments: [] })
  const draftTimestamp = Math.floor(Date.now() / 1_000).toString()
  const draftSignature = createHmac('sha256', webhook.secret).update(`${draftTimestamp}.${body}`).digest('base64url')
  const draftWebhookResponse = await page.request.post(`${gatewayBase}/gateway/v1/webhooks/${webhook.publicId}`, {
    data: body,
    headers: {
      'Content-Type': 'application/json',
      'Idempotency-Key': `m7-webhook-draft-${suffix}`,
      'X-Agentx-Signature': draftSignature,
      'X-Agentx-Timestamp': draftTimestamp,
    },
  })
  expect(draftWebhookResponse.status(), 'Webhook drafts must not activate before the next Application Deployment').toBe(404)
  expect((await draftWebhookResponse.json() as { code: string }).code).toBe('NOT_FOUND')

  const webhookDeployment = await request<ApplicationDeployment>(page, token, `/applications/${application.id}/deployments`, 'POST', {
    workflowVersionId: version!.id,
    environmentId: environment!.id,
    sessionVersionPolicy: 'pinned',
  })
  await waitApplicationDeployment(page, token, application.id, webhookDeployment.id)

  const timestamp = Math.floor(Date.now() / 1_000).toString()
  const signature = createHmac('sha256', webhook.secret).update(`${timestamp}.${body}`).digest('base64url')
  const webhookResponse = await page.request.post(`${gatewayBase}/gateway/v1/webhooks/${webhook.publicId}`, {
    data: body,
    headers: {
      'Content-Type': 'application/json',
      'Idempotency-Key': `m7-webhook-${suffix}`,
      'X-Agentx-Signature': signature,
      'X-Agentx-Timestamp': timestamp,
    },
  })
  await expectResponse(webhookResponse, 'signed webhook')
  expect(webhookResponse.status()).toBe(202)
  await approveInvocation(page, approvalPage, token, (await webhookResponse.json() as Invocation).id)

  const approvalsBeforeEvaluation = await request<PageResponse<Approval>>(page, token, '/approvals?pageSize=100')
  const existingApprovalIds = new Set(approvalsBeforeEvaluation.items.map(item => item.id))
  const evaluation = await beginApplicationEvaluation(page, token, { applicationId: application!.id, workflowId: workflow!.id, versionId: version!.id, name: `M7 Evaluation ${suffix}`, input: { question: 'evaluation-e2e', attachments: [] }, expectedOutput: referenceInvocation.outputs })
  let evaluationApproval: Approval | undefined
  await expect.poll(async () => {
    const approvals = await request<PageResponse<Approval>>(page, token, '/approvals?pageSize=100')
    evaluationApproval = approvals.items.find((item) =>
      item.workflowId === workflow!.id
      && item.status === 'pending'
      && !existingApprovalIds.has(item.id))
    return evaluationApproval?.id
  }, { timeout: 150_000, intervals: [500, 1_000, 2_000] }).toBeTruthy()
  await approveApproval(approvalPage, token, evaluationApproval!)
  let report: { status: string; runs: { cases: { executionId: string; status: string }[] }[] } | undefined
  await expect.poll(async () => {
    report = await request(page, token, evaluation.reportPath)
    return report!.status
  }, { timeout: 180_000, intervals: [750, 1_000, 2_000] }).toBe('completed')
  expect(report!.runs[0].cases[0].executionId).toBe(evaluationApproval!.executionId)
  expect(report!.runs[0].cases[0].status).toBe('passed')

  const capabilities = await request<Array<{ status: string }>>(page, token, '/runtime/capabilities')
  expect(capabilities.some((item) => item.status === 'ready')).toBeTruthy()
  const quotas = await request<Array<{ dimension: string; hardLimit: string }>>(page, token, '/runtime/quotas')
  const executionQuota = quotas.find((item) => item.dimension === 'execution_concurrency')
  expect(executionQuota).toBeTruthy()
  await request(page, token, '/runtime/quotas', 'PUT', { policies: [{ dimension: executionQuota!.dimension, hardLimit: executionQuota!.hardLimit, periodSeconds: null }] })
  const retention = await request<RetentionRun>(page, token, '/retention-runs', 'POST', {
    dryRun: true,
    artifactRetentionDays: 30,
    traceRetentionDays: 180,
    messageRetentionDays: 180,
    evaluationRetentionDays: 365,
  })
  await expect.poll(async () => (await request<RetentionRun[]>(page, token, '/retention-runs')).find((run) => run.id === retention.id)?.status, { timeout: 120_000, intervals: [500, 1_000, 2_000] }).toBe('completed')
  await page.goto('/runtime')
  await expect(page.getByRole('heading', { name: '运行状态' })).toBeVisible()
  await expect(page.getByText('运行配额')).toBeVisible()
  await expect(page.getByText('Worker 能力')).toBeVisible()
  await expect(page.getByText('数据保留')).toBeVisible()

  const workflowDetail = await request<Workflow>(page, token, `/workflows/${workflow!.id}`)
  const versionDetail = await request<WorkflowVersion>(page, token, `/workflows/${workflow!.id}/versions/${version!.id}`)
  const modelReference = versionDetail?.definition?.nodes?.flatMap((node) => node.resourceReferences ?? []).find((reference) => reference.resourceType === 'model')
  expect(workflowDetail.serviceIdentityId).toBeTruthy()
  expect(modelReference).toBeTruthy()
  const modelGrants = await request<Grant[]>(page, token, `/resources/model/${modelReference!.resourceId}/grants`)
  const existingModelGrant = modelGrants.find((grant) => grant.subjectType === 'workflow_service_identity' && grant.subjectId === workflowDetail.serviceIdentityId && grant.operation === modelReference!.operation)
  const modelGrant = existingModelGrant ?? await request<Grant>(page, token, `/resources/model/${modelReference!.resourceId}/grants`, 'POST', {
    subjectType: 'workflow_service_identity',
    subjectId: workflowDetail.serviceIdentityId,
    resourceVersionId: modelReference!.resourceVersionId ?? null,
    operation: modelReference!.operation,
  })
  await request(page, token, `/resources/model/${modelReference!.resourceId}/grants/${modelGrant.id}`, 'DELETE')
  try {
    const revokedResponse = await page.request.post(`${gatewayBase}/gateway/v1/applications/${application.slug}/invocations`, {
      data: { input: { question: 'revoked-grant-e2e', attachments: [] } },
      headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': `m7-revoked-grant-${suffix}` },
    })
    await expectResponse(revokedResponse, 'revoked resource invocation')
    const revokedInvocation = await waitInvocation(page, token, (await revokedResponse.json() as Invocation).id, true)
    expect(revokedInvocation.status).toBe('failed')
  } finally {
    const currentModelGrants = await request<Grant[]>(page, token, `/resources/model/${modelReference!.resourceId}/grants`)
    const grantWasRestored = currentModelGrants.some((grant) =>
      grant.subjectType === 'workflow_service_identity'
      && grant.subjectId === workflowDetail.serviceIdentityId
      && grant.operation === modelGrant.operation
      && (grant.resourceVersionId ?? null) === (modelGrant.resourceVersionId ?? null),
    )
    if (!grantWasRestored) {
      await request(page, token, `/resources/model/${modelReference!.resourceId}/grants`, 'POST', {
        subjectType: 'workflow_service_identity',
        subjectId: workflowDetail.serviceIdentityId,
        resourceVersionId: modelGrant.resourceVersionId ?? modelReference!.resourceVersionId ?? null,
        operation: modelGrant.operation,
      })
    }
  }
  const restoredResponse = await page.request.post(`${gatewayBase}/gateway/v1/applications/${application.slug}/invocations`, {
    data: { input: { question: 'restored-grant-e2e', attachments: [] } },
    headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': `m7-restored-grant-${suffix}` },
  })
  await expectResponse(restoredResponse, 'restored resource invocation')
  await approveInvocation(page, approvalPage, token, (await restoredResponse.json() as Invocation).id)

  const approvals = await request<PageResponse<Approval>>(page, token, '/approvals?pageSize=100')
  const completedApproval = approvals.items.find((item) => item.executionId === completed.executionId && item.status === 'decided')
  expect(completedApproval).toBeTruthy()
  const pages = [
    { key: 'playground', path: '/playground' },
    { key: 'application-trigger', path: `/applications/${triggerApplication.id}` },
    { key: 'evaluation', path: `/applications/${evaluation.applicationId}?tab=testing&testTab=runs&batchId=${evaluation.id}` },
    { key: 'approval', path: `/approvals/${completedApproval!.id}` },
    { key: 'runtime', path: '/runtime' },
  ]
  const viewports = [{ width: 1280, height: 800 }, { width: 1440, height: 900 }, { width: 1920, height: 1080 }]
  for (const locale of ['zh-CN', 'en-US'] as const) {
    await setLocale(page, locale)
    for (const theme of ['light', 'dark'] as const) {
      await setTheme(page, theme)
      for (const viewport of viewports) {
        await page.setViewportSize(viewport)
        for (const target of pages) {
          await page.goto(target.path)
          const main = page.locator('main')
          await expect(main).toBeVisible()
          await expect(main).not.toContainText(/(?:common|navigation|applications|approvals|datasets|evaluations|executions|runtime|studio)\.[A-Za-z]/)
          await expect(page.getByText('Invalid Date', { exact: true })).toHaveCount(0)
          if (locale === 'zh-CN') {
            for (const rawValue of ['active', 'completed', 'manual_upgrade', 'resource_missing_or_disabled']) {
              await expect(main.getByText(rawValue, { exact: true })).toHaveCount(0)
            }
          }
          expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBeTruthy()
          const unnamedButtons = await page.getByRole('button').evaluateAll((buttons) => buttons.filter((button) => {
            const element = button as HTMLElement
            return !(element.innerText.trim() || element.getAttribute('aria-label') || element.getAttribute('title'))
          }).length)
          expect(unnamedButtons, `${target.key} contains unnamed buttons`).toBe(0)
          await page.screenshot({ path: testInfo.outputPath(`${target.key}-${viewport.width}x${viewport.height}-${locale}-${theme}.png`), fullPage: true })
        }
      }
    }
  }

  await approvalPage.close()
})
