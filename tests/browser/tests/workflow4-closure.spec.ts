import { fixtureDraftId } from '../draft-fixtures'
import { expect, type APIResponse, type Page, test } from '@playwright/test'

const password = 'agentx-e2e-admin-password'
const company = 'Agentx E2E'
const gatewayBase = process.env.AGENTX_E2E_RUNTIME_URL ?? ''
const echoBaseUrl = process.env.AGENTX_E2E_ECHO_BASE_URL ?? 'http://echo-mcp:8090'

type Workflow = { id: string; name: string }
type Draft = { revision: number; definition: Definition; editorDocument: Record<string, unknown> }
type Version = { id: string; versionNumber: number }
type Application = { id: string; slug: string; apiKey: string }
type Deployment = { id: string; status: string }
type Environment = { id: string; code: string }
type TerminalError = { primaryError?: { code?: string; message?: string }; errors?: unknown[]; outputs?: Record<string, unknown> }
type Invocation = { id: string; executionId?: string; status: string; outputs?: Record<string, unknown>; error?: TerminalError }
type Execution = { id: string; status: string; parentExecutionId?: string; errorCode?: string }
type ExecutionPage = { items: Execution[] }
type Approval = { id: string; executionId: string; status: string; version: number }
type PageResponse<T> = { items: T[] }
type Definition = {
  schemaVersion: '8.0'
  start: { inputs: Record<string, unknown>; contexts: Record<string, unknown> }
  nodes: Array<Record<string, unknown> & { id: string; key: string; type: string }>
  connections: Array<{ id: string; sourceNodeId: string; sourceHandle: string; targetNodeId: string; targetHandle: string; order: number }>
  end: { completion?: 'first_return' | 'all_complete'; outputs: Record<string, unknown>; error?: { outputs: Record<string, unknown> } }
  settings: Record<string, unknown>
}

async function login(page: Page) {
  const bootstrapStatus = await page.request.get('/api/v1/bootstrap/status')
  await expectResponse(bootstrapStatus, 'bootstrap status')
  if (((await bootstrapStatus.json()) as { required: boolean }).required) {
    await page.goto('/setup')
    await page.getByLabel('公司名称').fill(company)
    await page.getByLabel('用户名').fill('admin')
    await page.getByLabel('管理员姓名').fill('E2E Admin')
    await page.getByLabel('密码').fill(password)
    const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/bootstrap') && value.request().method() === 'POST')
    await page.getByRole('button', { name: '初始化并进入工作台' }).click()
    await expect(page).toHaveURL(/\/$/)
    return ((await (await response).json()) as { accessToken: string }).accessToken
  }
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill(password)
  const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/auth/login') && value.request().method() === 'POST')
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
  return ((await (await response).json()) as { accessToken: string }).accessToken
}

async function expectResponse(response: APIResponse, label: string) {
  if (!response.ok()) throw new Error(`${label}: ${response.status()} ${await response.text()}`)
}

async function request<T>(page: Page, token: string, path: string, method = 'GET', body?: unknown, gateway = false, headers: Record<string, string> = {}): Promise<T> {
  const response = await page.request.fetch(`${gateway ? `${gatewayBase}/gateway/v1` : '/api/v1'}${path}`, {
    method,
    data: body,
    headers: { Authorization: `Bearer ${token}`, ...headers },
  })
  await expectResponse(response, path)
  const text = await response.text()
  return (text ? JSON.parse(text) : undefined) as T
}

function node(id: string, type: string, parameters: Record<string, unknown>, extra: Record<string, unknown> = {}) {
  return {
    id,
    key: id.replaceAll('-', '_'),
    type,
    typeVersion: 1,
    name: id,
    disabled: false,
    parameters,

    contextWrites: [],
    resourceReferences: [],
    settings: {},
    ...extra,
  }
}

const reference = (namespace: 'inputs' | 'outputs' | 'contexts' | 'item', path: string[], sourceNodeId?: string) => ({
  kind: 'reference',
  selector: { namespace, sourceNodeId, port: sourceNodeId ? 'main' : undefined, run: { kind: 'current' }, item: { kind: 'current' }, path },
  missingPolicy: { kind: 'error' },
})
const textTemplate = (text: string) => ({ kind: 'template', segments: [{ kind: 'text', text }] })
const literal = (value: unknown) => ({ kind: 'literal', value })
const objectBinding = (fields: Record<string, unknown>) => ({ kind: 'object', fields })
const arrayBinding = (items: unknown[]) => ({ kind: 'array', items })

async function createWorkflow(page: Page, token: string, name: string, definition: Definition) {
  const workflow = await request<Workflow>(page, token, '/workflows', 'POST', {
    name,
    description: 'Workflow 5.0 Kubernetes closure',
    visibility: 'company',
  })
  const draft = await request<Draft>(page, token, `/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`)
  const saved = await request<Draft>(page, token, `/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`, 'PUT', {
    expectedRevision: draft.revision,
    definition,
    editorDocument: {
      ...draft.editorDocument,
      nodeLayouts: definition.nodes.map((value, index) => ({ nodeId: value.id, x: 120 + index * 260, y: 180 })),
      edges: definition.connections.map((value) => ({ edgeId: value.id })),
    },
  })
  const version = await request<Version>(page, token, `/workflows/${workflow.id}/versions`, 'POST', { draftId: await fixtureDraftId(page, workflow.id),  draftRevision: saved.revision })
  return { workflow, version, draft: saved }
}

async function reviseWorkflow(page: Page, token: string, workflowId: string, definition: Definition) {
  const draft = await request<Draft>(page, token, `/workflows/${workflowId}/drafts/${await fixtureDraftId(page, workflowId)}`)
  const saved = await request<Draft>(page, token, `/workflows/${workflowId}/drafts/${await fixtureDraftId(page, workflowId)}`, 'PUT', {
    expectedRevision: draft.revision,
    definition,
    editorDocument: {
      ...draft.editorDocument,
      nodeLayouts: definition.nodes.map((value, index) => ({ nodeId: value.id, x: 120 + index * 260, y: 180 })),
      edges: definition.connections.map((value) => ({ edgeId: value.id })),
    },
  })
  return request<Version>(page, token, `/workflows/${workflowId}/versions`, 'POST', { draftId: await fixtureDraftId(page, workflowId),  draftRevision: saved.revision })
}

async function deployApplication(page: Page, token: string, workflow: Workflow, version: Version, environmentId: string, slug: string) {
  await request(page, token, `/workflows/${workflow.id}/deployments`, 'POST', { workflowVersionId: version.id, environmentId })
  const application = await request<Omit<Application, 'apiKey'>>(page, token, '/applications', 'POST', {
    workflowId: workflow.id,
    name: `${workflow.name} Application`,
    slug,
    description: 'Workflow 5.0 E2E application',
    visibility: 'company',
  })
  const apiKey = await request<{ secret: string }>(page, token, `/applications/${application.id}/api-keys`, 'POST', {
    name: 'Workflow 5.0 E2E',
  })
  const deployment = await request<Deployment>(page, token, `/applications/${application.id}/deployments`, 'POST', {
    workflowVersionId: version.id,
    environmentId,
    sessionVersionPolicy: 'pinned',
  })
  await expect.poll(async () => {
    const deployments = await request<Deployment[]>(page, token, `/applications/${application.id}/deployments`)
    const status = deployments.find((value) => value.id === deployment.id)?.status
    if (status === 'rejected') throw new Error(`application deployment ${deployment.id} was rejected`)
    return status
  }, { timeout: 180_000, intervals: [500, 1_000, 2_000] }).toBe('active')
  return { ...application, apiKey: apiKey.secret }
}

async function waitInvocation(page: Page, token: string, id: string) {
  let current: Invocation | undefined
  await expect.poll(async () => {
    current = await request<Invocation>(page, token, `/invocations/${id}`, 'GET', undefined, true)
    return ['completed', 'failed', 'cancelled'].includes(current.status)
  }, { timeout: 180_000, intervals: [250, 500, 1_000, 2_000] }).toBeTruthy()
  return current!
}

async function waitExecution(page: Page, token: string, id: string, statuses: string[]) {
  let current: Execution | undefined
  await expect.poll(async () => {
    current = await request<Execution>(page, token, `/executions/${id}`)
    return statuses.includes(current.status)
  }, { timeout: 180_000, intervals: [250, 500, 1_000, 2_000] }).toBeTruthy()
  return current!
}

const counterContext = {
  counter: {
    schema: { type: 'number' },
    default: 0,
    mutable: true,
    sensitive: false,
    scope: 'execution_tree',
    mergePolicy: 'increment',
    clientWritable: false,
  },
}

test('Workflow 5.0 closes Composite, Context, Package, Multipart and cancellation contracts', async ({ page }) => {
  const token = await login(page)
  const actor = await request<{ id: string }>(page, token, '/auth/me')
  const suffix = Date.now()
  const environment = (await request<Environment[]>(page, token, '/environments')).find((value) => value.code === 'development')
  expect(environment).toBeTruthy()

  const childDefinition: Definition = {
    schemaVersion: '8.0',
    start: {
      inputs: { type: 'object', required: ['question'], properties: { question: { type: 'string' } }, additionalProperties: false },
      contexts: counterContext,
    },
    nodes: [node('answer', 'set', { values: objectBinding({ answer: literal('v1:hello') }), keepOnlySet: true }, {
      contextWrites: [{ operation: 'increment', path: 'counter', value: { kind: 'literal', value: 1 } }],
    }),
      { id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true, parameters: { outputs: { answer: reference('outputs', ['answer'], 'answer') }, errorOutputs: {} }, contextWrites: [], resourceReferences: [], settings: {} },
    ],
    connections: [
      { id: 'start-answer', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'answer', targetHandle: 'main', order: 0 },
      { id: 'answer-end', sourceNodeId: 'answer', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 },
    ],
    end: { outputs: { answer: { schema: { type: 'string' }, required: true, sensitive: false } } },
    settings: { executionOrder: 'deterministic' },
  }
  const child = await createWorkflow(page, token, `W4 Child ${suffix}`, childDefinition)

  const parentDefinition: Definition = {
    schemaVersion: '8.0',
    start: {
      inputs: {
        type: 'object',
        required: ['question', 'attachments'],
        properties: {
          question: { type: 'string', minLength: 2 },
          prefix: { type: 'string', default: 'default-applied' },
          attachments: {
            type: 'array',
            minItems: 1,
            maxItems: 2,
            items: { type: 'object' },
            'x-agentx-artifact': true,
            'x-agentx-artifact-array': true,
            'x-agentx-content-types': ['text/plain'],
            'x-agentx-max-size-bytes': 100,
            'x-agentx-max-total-size-bytes': 100,
          },
        },
        additionalProperties: false,
      },
      contexts: counterContext,
    },
    nodes: [
      node('child', 'sub_workflow', { workflowVersionId: child.version.id, inputs: objectBinding({ question: reference('inputs', ['question']) }) }),
      node('summary', 'set', { values: objectBinding({ answer: reference('outputs', ['answer'], 'child'), counter: reference('contexts', ['counter']) }), keepOnlySet: true }),
      { id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true, parameters: { outputs: {
        answer: reference('outputs', ['answer'], 'summary'),
        counter: reference('outputs', ['counter'], 'summary'),
        attachments: reference('inputs', ['attachments']),
        prefix: reference('inputs', ['prefix']),
      }, errorOutputs: {} }, contextWrites: [], resourceReferences: [], settings: {} },
    ],
    connections: [
      { id: 'start-child', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'child', targetHandle: 'main', order: 0 },
      { id: 'child-summary', sourceNodeId: 'child', sourceHandle: 'main', targetNodeId: 'summary', targetHandle: 'main', order: 0 },
      { id: 'summary-end', sourceNodeId: 'summary', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 },
    ],
    end: {
      outputs: {
        answer: { schema: { type: 'string' }, required: true, sensitive: false },
        counter: { schema: { type: 'number' }, required: true, sensitive: false },
        attachments: { schema: { type: 'array' }, required: true, sensitive: false },
        prefix: { schema: { type: 'string' }, required: true, sensitive: false },
      },
    },
    settings: { executionOrder: 'deterministic' },
  }
  const parent = await createWorkflow(page, token, `W4 Parent ${suffix}`, parentDefinition)

  const childV2 = structuredClone(childDefinition)
  ;(childV2.nodes[0].parameters as Record<string, unknown>).values = objectBinding({ answer: literal('v2:hello') })
  const secondChildVersion = await reviseWorkflow(page, token, child.workflow.id, childV2)
  expect(secondChildVersion.versionNumber).toBe(2)

  const application = await deployApplication(page, token, parent.workflow, parent.version, environment!.id, `w4-parent-${suffix}`)
  const invalidInput = await page.request.post(`${gatewayBase}/gateway/v1/workflows/${application.slug}/invoke`, {
    headers: { Authorization: `Bearer ${application.apiKey}`, 'Idempotency-Key': `w4-invalid-${suffix}` },
    data: { input: { question: 'x', attachments: [] }, responseMode: 'async' },
  })
  expect(invalidInput.status()).toBe(400)
  expect(await invalidInput.json()).toMatchObject({ code: 'INPUT_SCHEMA_VALIDATION_FAILED' })

  const uploadArtifact = async (id: string, file: { name: string; type: string; contents: string }, index = 0) => {
    const response = await page.request.post(`${gatewayBase}/gateway/v1/artifacts`, {
      headers: { Authorization: `Bearer ${application.apiKey}`, 'Idempotency-Key': `w4-artifact-${id}-${index}-${suffix}` },
      multipart: { file: { name: file.name, mimeType: file.type, buffer: Buffer.from(file.contents) } },
    })
    await expectResponse(response, `Artifact upload ${id}/${index}`)
    return { ...await response.json() as { artifactId: string; contentType: string; sizeBytes: number; sha256: string }, type: 'file', fileName: file.name }
  }
  const rejectFiles = async (id: string, files: Array<{ name: string; type: string; contents: string }>, code: string) => {
    const attachments = await Promise.all(files.map((file, index) => uploadArtifact(id, file, index)))
    const response = await page.request.post(`${gatewayBase}/gateway/v1/workflows/${application.slug}/invoke`, {
      headers: { Authorization: `Bearer ${application.apiKey}`, 'Idempotency-Key': `w4-${id}-${suffix}` },
      data: { input: { question: 'hello', attachments }, responseMode: 'async' },
    })
    expect(response.status()).toBe(400)
    expect(await response.json()).toMatchObject({ code })
  }
  await rejectFiles('mime', [{ name: 'question.json', type: 'application/json', contents: '{}' }], 'ARTIFACT_CONTENT_TYPE_NOT_ALLOWED')
  await rejectFiles('file-size', [{ name: 'large.txt', type: 'text/plain', contents: 'x'.repeat(101) }], 'ARTIFACT_TOO_LARGE')
  await rejectFiles('total-size', [
    { name: 'first.txt', type: 'text/plain', contents: 'x'.repeat(60) },
    { name: 'second.txt', type: 'text/plain', contents: 'x'.repeat(60) },
  ], 'ARTIFACT_TOTAL_SIZE_EXCEEDED')
  await rejectFiles('file-count', [
    { name: 'first.txt', type: 'text/plain', contents: '1' },
    { name: 'second.txt', type: 'text/plain', contents: '2' },
    { name: 'third.txt', type: 'text/plain', contents: '3' },
  ], 'INPUT_SCHEMA_VALIDATION_FAILED')

  const artifact = await uploadArtifact('success', { name: 'question.txt', type: 'text/plain', contents: 'workflow-4-file' })
  const artifactInvocation = await page.request.post(`${gatewayBase}/gateway/v1/workflows/${application.slug}/invoke`, {
    headers: { Authorization: `Bearer ${application.apiKey}`, 'Idempotency-Key': `w4-multipart-${suffix}` },
    data: { input: { question: 'hello', attachments: [artifact] }, responseMode: 'sync' },
  })
  await expectResponse(artifactInvocation, 'Artifact reference invoke')
  const invocation = await artifactInvocation.json() as Invocation
  const completed = invocation.status === 'completed' ? invocation : await waitInvocation(page, application.apiKey, invocation.id)
  expect(completed.status).toBe('completed')
  expect(completed.outputs).toMatchObject({ answer: 'v1:hello', counter: 1, prefix: 'default-applied' })
  expect(completed.outputs?.attachments).toEqual([
    expect.objectContaining({ artifactId: expect.stringMatching(/^[0-9a-f-]{36}$/), fileName: 'question.txt', contentType: 'text/plain' }),
  ])

  const exported = await request<Record<string, unknown>>(page, token, `/workflows/${parent.workflow.id}/drafts/${await fixtureDraftId(page, parent.workflow.id)}/export`)
  const imported = await request<{ workflowId: string }>(page, token, '/workflows/import', 'POST', {
    name: `W4 Imported ${suffix}`,
    description: 'Cross-project package contract',
    visibility: 'company',
    package: exported,
  })
  const importedDraft = await request<Draft>(page, token, `/workflows/${imported.workflowId}/drafts/${await fixtureDraftId(page, imported.workflowId)}`)
  expect(importedDraft.definition.schemaVersion).toBe('8.0')
  expect(importedDraft.definition.nodes[0].type).toBe('sub_workflow')

  const recursiveDefinition = structuredClone(childV2)
  recursiveDefinition.nodes = [
    node('parent', 'sub_workflow', {
      workflowVersionId: parent.version.id,
      inputs: objectBinding({ question: reference('inputs', ['question']), attachments: arrayBinding([]) }),
    }),
    { id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true, parameters: { outputs: {}, errorOutputs: {} }, contextWrites: [], resourceReferences: [], settings: {} },
  ]
  recursiveDefinition.connections = [
    { id: 'start-parent', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'parent', targetHandle: 'main', order: 0 },
    { id: 'parent-end', sourceNodeId: 'parent', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 },
  ]
  recursiveDefinition.end.outputs = {}
  const recursiveDraft = await request<Draft>(page, token, `/workflows/${child.workflow.id}/drafts/${await fixtureDraftId(page, child.workflow.id)}`)
  const recursiveSave = await page.request.put(`/api/v1/workflows/${child.workflow.id}/drafts/${await fixtureDraftId(page, child.workflow.id)}`, {
    headers: { Authorization: `Bearer ${token}` },
    data: {
      expectedRevision: recursiveDraft.revision,
      definition: recursiveDefinition,
      editorDocument: { ...recursiveDraft.editorDocument, nodeLayouts: [{ nodeId: 'parent', x: 120, y: 180 }], edges: [] },
    },
  })
  expect(recursiveSave.status()).toBe(422)
  expect(await recursiveSave.json()).toMatchObject({ code: 'RECURSIVE_SUBWORKFLOW' })

  const slowChildDefinition: Definition = {
    schemaVersion: '8.0',
    start: { inputs: { type: 'object', properties: {}, additionalProperties: false }, contexts: {} },
    nodes: [node('slow_approval', 'approval', { title: textTemplate('Slow Approval'), timeoutMs: 60_000, candidateUserId: actor.id }),
      { id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true, parameters: { outputs: {}, errorOutputs: {} }, contextWrites: [], resourceReferences: [], settings: {} },
    ],
    connections: [
      { id: 'start-approval', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'slow_approval', targetHandle: 'main', order: 0 },
      { id: 'approval-end', sourceNodeId: 'slow_approval', sourceHandle: 'decision:approved', targetNodeId: 'exit', targetHandle: 'main', order: 0 },
      { id: 'approval-timeout-end', sourceNodeId: 'slow_approval', sourceHandle: 'timed_out', targetNodeId: 'exit', targetHandle: 'main', order: 0 },
    ],
    end: { outputs: {} },
    settings: { executionOrder: 'deterministic' },
  }
  const slowChild = await createWorkflow(page, token, `W4 Slow Child ${suffix}`, slowChildDefinition)
  const slowParentDefinition: Definition = {
    schemaVersion: '8.0',
    start: { inputs: { type: 'object', properties: {}, additionalProperties: false }, contexts: {} },
    nodes: [node('slow_child', 'sub_workflow', { workflowVersionId: slowChild.version.id, inputs: objectBinding({}) }, {
      settings: { timeoutMs: 1_500, retryOnFail: false, maxTries: 1 },
    }),
      { id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true, parameters: { outputs: { status: literal('timeout-test') }, errorOutputs: {} }, contextWrites: [], resourceReferences: [], settings: {} },
    ],
    connections: [
      { id: 'start-slow-child', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'slow_child', targetHandle: 'main', order: 0 },
      { id: 'slow-child-end', sourceNodeId: 'slow_child', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 },
    ],
    end: {
      outputs: {
        status: { schema: { type: 'string' }, required: true, sensitive: false },
      },
    },
    settings: { executionOrder: 'deterministic' },
  }
  const slowParent = await createWorkflow(page, token, `W4 Slow Parent ${suffix}`, slowParentDefinition)
  const slowApplication = await deployApplication(page, token, slowParent.workflow, slowParent.version, environment!.id, `w4-slow-${suffix}`)
  const slowInvoke = await request<Invocation>(page, slowApplication.apiKey, `/workflows/${slowApplication.slug}/invoke`, 'POST', { input: {}, responseMode: 'async' }, true, {
    'Idempotency-Key': `w4-slow-${suffix}`,
  })
  const slowTerminal = await waitInvocation(page, slowApplication.apiKey, slowInvoke.id)
  expect(slowTerminal.status).toBe('failed')
  expect(slowTerminal.error?.primaryError?.code).toBeTruthy()
  const parentExecution = await waitExecution(page, token, slowTerminal.executionId!, ['failed'])
  let childExecution: Execution | undefined
  await expect.poll(async () => {
    const executions = await request<ExecutionPage>(page, token, '/executions?limit=100')
    childExecution = executions.items.find((value) => value.parentExecutionId === parentExecution.id)
    return childExecution?.status
  }, { timeout: 60_000 }).toBe('cancelled')
})

test('Workflow 5.0 declarative HTTP consumes request parameters and returns its body as text', async ({ page }) => {
  const token = await login(page)
  const suffix = Date.now()
  const environment = (await request<Environment[]>(page, token, '/environments')).find((value) => value.code === 'development')!
  const definition: Definition = {
    schemaVersion: '8.0',
    start: { inputs: { type: 'object', properties: {}, additionalProperties: false }, contexts: {} },
    nodes: [node('http', 'declarative_http', {
      method: 'POST',
      url: textTemplate(`${echoBaseUrl}/v1/chat/completions`),
      query: [],
      headers: [
        { name: 'Authorization', value: textTemplate('Bearer m5-model-secret') },
        { name: 'Content-Type', value: textTemplate('application/json') },
      ],
      body: {
        kind: 'object',
        fields: {
          model: literal('contract-http'),
          messages: {
            kind: 'array',
            items: [
              { kind: 'object', fields: { role: literal('system'), content: literal('Declarative HTTP contract') } },
              { kind: 'object', fields: { role: literal('user'), content: literal('Return the fixture response') } },
            ],
          },
        },
      },
    }),
      { id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true, parameters: { outputs: {
        status_code: reference('outputs', ['statusCode'], 'http'),
        body_text: reference('outputs', ['body'], 'http'),
      }, errorOutputs: {} }, contextWrites: [], resourceReferences: [], settings: {} },
    ],
    connections: [
      { id: 'start-http', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'http', targetHandle: 'main', order: 0 },
      { id: 'http-end', sourceNodeId: 'http', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 },
    ],
    end: {
      outputs: {
        status_code: { schema: { type: 'integer' }, required: true, sensitive: false },
        body_text: { schema: { type: 'string' }, required: true, sensitive: false },
      },
    },
    settings: { executionOrder: 'deterministic' },
  }
  const workflow = await createWorkflow(page, token, `W4 Declarative HTTP ${suffix}`, definition)
  const application = await deployApplication(page, token, workflow.workflow, workflow.version, environment.id, `w4-http-${suffix}`)
  const invocation = await request<Invocation>(page, application.apiKey, `/workflows/${application.slug}/invoke`, 'POST', {
    input: {}, responseMode: 'sync',
  }, true, { 'Idempotency-Key': `w4-http-${suffix}` })
  const terminal = invocation.status === 'completed' ? invocation : await waitInvocation(page, application.apiKey, invocation.id)

  expect(terminal.status).toBe('completed')
  expect(terminal.outputs?.status_code).toBe(200)
  expect(terminal.outputs?.body_text).toBe('{"choices":[{"finish_reason":"stop","index":0,"message":{"content":"M5 Agent completed after the MCP tool result","role":"assistant"}}],"id":"m5-completion","model":"contract-http","object":"chat.completion","usage":{"completion_tokens":9,"prompt_tokens":24,"total_tokens":45}}')
  expect(terminal.executionId).toBeTruthy()
  let endBoundary: { spanId: string } | undefined
  await expect.poll(async () => {
    const trace = await request<{ spans: Array<{ spanId: string; spanKind: string; spanName: string; hasDetails: boolean }> }>(page, token, `/executions/${terminal.executionId}/trace?limit=100`)
    endBoundary = trace.spans.find((span) => span.spanKind === 'boundary' && span.spanName === 'End' && span.hasDetails)
    return endBoundary?.spanId
  }, { timeout: 60_000, intervals: [500, 1_000, 2_000] }).toBeTruthy()
  const endDetail = await request<{
    contents: Array<{ eventId: string; kind: string; preview?: { records?: Array<{ targetPath: string; sourceType: string; mode: string; resultBytes: number }> } }>
    events: Array<{ eventId: string; attributes?: { diagnostic?: string; conversionCount?: number } }>
  }>(page, token, `/executions/${terminal.executionId}/trace/spans/${endBoundary!.spanId}`)
  expect(endDetail.contents.find((content) => content.kind === 'conversion_record')?.preview?.records).toContainEqual(expect.objectContaining({
    sourceType: 'number', mode: 'schema:integer', targetPath: 'exit.exit.outputs.status_code',
  }))
  const endDetailJson = JSON.stringify(endDetail)
  expect(endDetailJson).not.toContain('m5-model-secret')
  expect(endDetailJson).toContain('M5 Agent completed after the MCP tool result')
})

test('Workflow 5.0 turns concurrent Session Context CAS conflicts into a terminal failure', async ({ page }) => {
  const token = await login(page)
  const actor = await request<{ id: string }>(page, token, '/auth/me')
  const suffix = Date.now()
  const environment = (await request<Environment[]>(page, token, '/environments')).find((value) => value.code === 'development')!
  const definition: Definition = {
    schemaVersion: '8.0',
    start: {
      inputs: { type: 'object', properties: {}, additionalProperties: false },
      contexts: {
        session_counter: {
          schema: { type: 'number' }, default: 0, mutable: true, sensitive: false,
          scope: 'session', mergePolicy: 'reject_conflict', clientWritable: false,
        },
      },
    },
    nodes: [
      node('approval', 'approval', {
        title: textTemplate(`W4 Session CAS barrier ${suffix}`),
        timeoutMs: 300_000,
        candidateUserId: actor.id,
      }),
      node('write', 'set', { values: objectBinding({ counter: reference('contexts', ['session_counter']) }), keepOnlySet: true }, {
        contextWrites: [{ operation: 'increment', path: 'session_counter', value: { kind: 'literal', value: 1 } }],
      }),
      { id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true, parameters: { outputs: { counter: reference('contexts', ['session_counter']) }, errorOutputs: {} }, contextWrites: [], resourceReferences: [], settings: {} },
    ],
    connections: [
      { id: 'start-approval', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'approval', targetHandle: 'main', order: 0 },
      { id: 'approval-write', sourceNodeId: 'approval', sourceHandle: 'decision:approved', targetNodeId: 'write', targetHandle: 'main', order: 0 },
      { id: 'write-end', sourceNodeId: 'write', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 },
    ],
    end: { outputs: { counter: { schema: { type: 'number' }, required: true, sensitive: false } } },
    settings: { executionOrder: 'parallel' },
  }
  const workflow = await createWorkflow(page, token, `W4 Session CAS ${suffix}`, definition)
  const application = await deployApplication(page, token, workflow.workflow, workflow.version, environment.id, `w4-session-${suffix}`)
  const session = await request<{ id: string }>(page, application.apiKey, `/applications/${application.slug}/sessions`, 'POST', {}, true, {
    'Idempotency-Key': `w4-session-create-${suffix}`,
  })
  let invocationIndex = 0
  const invoke = () => request<Invocation>(page, application.apiKey, `/workflows/${application.slug}/invoke`, 'POST', { input: {}, sessionId: session.id }, true, {
    'Idempotency-Key': `w4-session-${suffix}-${invocationIndex++}`,
  })
  const [first, second] = await Promise.all([invoke(), invoke()])
  expect(first.executionId).toBeTruthy()
  expect(second.executionId).toBeTruthy()
  const executionIds = new Set([first.executionId!, second.executionId!])
  let approvals: Approval[] = []
  await expect.poll(async () => {
    const result = await request<PageResponse<Approval>>(page, token, '/approvals?pageSize=100')
    approvals = result.items.filter((approval) => executionIds.has(approval.executionId))
    return approvals.length === 2 && approvals.every((approval) => approval.status === 'pending')
  }, { timeout: 180_000, intervals: [250, 500, 1_000, 2_000] }).toBeTruthy()
  for (const approval of approvals) {
    const claimed = await request<Approval>(page, token, `/approvals/${approval.id}/claim`, 'POST', { version: approval.version })
    await request<Approval>(page, token, `/approvals/${approval.id}/decide`, 'POST', { version: claimed.version, decisionId: 'approved', reason: null, idempotencyKey: crypto.randomUUID() })
  }
  const terminals = await Promise.all([waitInvocation(page, application.apiKey, first.id), waitInvocation(page, application.apiKey, second.id)])
  expect(terminals.map((value) => value.status).sort()).toEqual(['completed', 'failed'])
  expect(terminals.find((value) => value.status === 'failed')?.error?.primaryError?.code).toBe('SESSION_CONTEXT_VERSION_CONFLICT')
})

test('Workflow 5.0 propagates fail-fast, collected and Composite End errors', async ({ page }) => {
  const token = await login(page)
  const suffix = Date.now()
  const environment = (await request<Environment[]>(page, token, '/environments')).find((value) => value.code === 'development')!
  const terminalOutput = {
    reported_code: { schema: { type: 'string' }, required: true, sensitive: false },
  }
  // Wiring is the policy: each failure node wires its error port to the exit
  // error port, which routes the failure item and fails the execution.
  const errorDefinition = (codes: string[]): Definition => ({
    schemaVersion: '8.0',
    start: { inputs: { type: 'object', properties: {}, additionalProperties: false }, contexts: {} },
    nodes: [
      ...codes.map((_, index) => node(`failure_${index}`, 'declarative_http', {
        method: 'GET', url: textTemplate('https://127.0.0.1/blocked'), query: [], headers: [],
      })),
      { id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true, parameters: { outputs: {}, errorOutputs: { reported_code: reference('item', ['code']) } }, contextWrites: [], resourceReferences: [], settings: {} },
    ],
    connections: [
      ...codes.flatMap((_, index) => [
        { id: `start-failure-${index}`, sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: `failure_${index}`, targetHandle: 'main', order: index },
        { id: `failure-${index}-main-end`, sourceNodeId: `failure_${index}`, sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: index },
        { id: `failure-${index}-end`, sourceNodeId: `failure_${index}`, sourceHandle: 'error', targetNodeId: 'exit', targetHandle: 'error', order: index },
      ]),
    ],
    end: { completion: 'first_return', outputs: {}, error: { outputs: terminalOutput } },
    settings: { executionOrder: 'parallel' },
  })

  const invokeFailure = async (label: string, definition: Definition) => {
    const slugLabel = label.toLowerCase().replaceAll(' ', '-')
    const workflow = await createWorkflow(page, token, `W4 ${label} ${suffix}`, definition)
    const application = await deployApplication(page, token, workflow.workflow, workflow.version, environment.id, `w4-${slugLabel}-${suffix}`)
    const invocation = await request<Invocation>(page, application.apiKey, `/workflows/${application.slug}/invoke`, 'POST', { input: {}, responseMode: 'async' }, true, {
      'Idempotency-Key': `w4-${slugLabel}-${suffix}`,
    })
    return { workflow, terminal: await waitInvocation(page, application.apiKey, invocation.id) }
  }

  const failFast = await invokeFailure('Fail Fast', errorDefinition(['FAIL_FAST']))
  expect(failFast.terminal.status).toBe('failed')
  const failFastCode = failFast.terminal.error?.primaryError?.code
  expect(failFastCode).toEqual(expect.any(String))
  expect(failFast.terminal.error?.outputs?.reported_code).toBe(failFastCode)


  const child = await createWorkflow(page, token, `W4 Error Child ${suffix}`, errorDefinition(['CHILD_FAILED']))
  const parentDefinition: Definition = {
    schemaVersion: '8.0',
    start: { inputs: { type: 'object', properties: {}, additionalProperties: false }, contexts: {} },
    nodes: [
      node('child', 'sub_workflow', { workflowVersionId: child.version.id, inputs: objectBinding({}) }),
      { id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true, parameters: { outputs: {}, errorOutputs: { reported_code: reference('item', ['code']) } }, contextWrites: [], resourceReferences: [], settings: {} },
    ],
    connections: [
      { id: 'start-child', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'child', targetHandle: 'main', order: 0 },
      { id: 'child-main-end', sourceNodeId: 'child', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 },
      { id: 'child-error-end', sourceNodeId: 'child', sourceHandle: 'error', targetNodeId: 'exit', targetHandle: 'error', order: 0 },
    ],
    end: {
      completion: 'first_return',
      outputs: {},
      error: { outputs: terminalOutput },
    },
    settings: { executionOrder: 'parallel' },
  }
  const parent = await invokeFailure('Composite Error', parentDefinition)
  expect(parent.terminal).toMatchObject({ status: 'failed', error: { primaryError: { code: expect.any(String) } } })
  const parentExecution = await waitExecution(page, token, parent.terminal.executionId!, ['failed'])
  const executions = await request<ExecutionPage>(page, token, '/executions?limit=100')
  expect(executions.items).toContainEqual(expect.objectContaining({ parentExecutionId: parentExecution.id, status: 'failed' }))
})
