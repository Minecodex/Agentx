import { fixtureVersionId, fixtureDraftId, fixtureDraftIdKnown } from '../draft-fixtures'
import { expect, type APIResponse, type Page, test } from '@playwright/test'
import { sandboxImageDigest } from './workflow-studio-editor-helpers'

const password = 'agentx-e2e-admin-password'
const echoBaseUrl = process.env.AGENTX_E2E_ECHO_BASE_URL ?? 'http://echo-mcp:8090'

type Execution = { id: string; status: string; errorCode?: string | null }
type NodeRun = { nodeId: string; status: string; errorCode?: string | null; output?: Record<string, Array<{ json: Record<string, unknown> }>> }

type Definition = {
  schemaVersion: '8.0'
  start: { inputs: Record<string, unknown>; contexts: Record<string, unknown> }
  nodes: Array<Record<string, unknown>>
  connections: Array<{ id: string; sourceNodeId: string; sourceHandle: string; targetNodeId: string; targetHandle: string; order: number }>
  end: Record<string, unknown>
  settings: { executionOrder: 'deterministic' | 'parallel'; activationBudget: number }
}

async function login(page: Page) {
  const bootstrapStatus = await page.request.get('/api/v1/bootstrap/status')
  if (!bootstrapStatus.ok()) throw new Error(`bootstrap status: ${bootstrapStatus.status()} ${await bootstrapStatus.text()}`)
  if (((await bootstrapStatus.json()) as { required: boolean }).required) {
    await page.goto('/setup')
    await page.getByLabel('公司名称').fill('Agentx E2E')
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

async function request<T>(page: Page, token: string, path: string, method = 'GET', body?: unknown, headers: Record<string, string> = {}): Promise<T> {
  const response = await page.request.fetch(`/api/v1${path}`, { method, data: body, headers: { Authorization: `Bearer ${token}`, ...headers } })
  await expectResponse(response, path)
  const bytes = await response.body()
  return bytes.length ? JSON.parse(bytes.toString()) as T : undefined as T
}

async function expectResponse(response: APIResponse, label: string) {
  if (!response.ok()) throw new Error(`${label}: ${response.status()} ${await response.text()}`)
}

async function createWorkflow(page: Page, token: string, name: string) {
  const workflow = await request<{ id: string }>(page, token, '/workflows', 'POST', {
    name,
    description: 'M6 local built-in Kubernetes E2E',
    visibility: 'company',
  })
  return workflow.id
}

async function saveDefinition(page: Page, token: string, workflowId: string, definition: Definition) {
  const draft = await request<{ revision: number; editorDocument: Record<string, unknown> }>(page, token, `/workflows/${workflowId}/drafts/${await fixtureDraftId(page, workflowId)}`)
  await request<{ revision: number }>(page, token, `/workflows/${workflowId}/drafts/${await fixtureDraftId(page, workflowId)}`, 'PUT', {
    expectedRevision: draft.revision,
    definition,
    editorDocument: draft.editorDocument,
  })
}

async function runFromStudio(page: Page, token: string, workflowId: string, input: Record<string, unknown>, expectedStatus: 'succeeded' | 'failed') {
  await page.goto(`/workflows/${workflowId}/versions/${await fixtureVersionId(page, workflowId)}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  const accepted = page.waitForResponse((value) => value.url().endsWith(`/workflows/${workflowId}/drafts/${fixtureDraftIdKnown(workflowId)}/debug-executions`) && value.request().method() === 'POST', { timeout: 60_000 })
  await page.locator('header').getByRole('button', { name: '运行', exact: true }).click()
  const dialog = page.getByRole('dialog').filter({ has: page.getByRole('button', { name: /开始运行|Run/ }) }).first()
  if (await dialog.waitFor({ state: 'visible', timeout: 1_000 }).then(() => true).catch(() => false)) {
    const inputField = dialog.locator('input:not([type="hidden"]), textarea').first()
    if (await inputField.isVisible().catch(() => false)) {
      const value = Object.values(input)[0]
      await inputField.fill(typeof value === 'object' ? JSON.stringify(value) : String(value))
    }
    await dialog.getByRole('button', { name: /开始运行|Run/ }).click()
  }
  const response = await accepted
  if (response.status() !== 202) throw new Error(`debug execution: ${response.status()} ${await response.text()}`)
  const executionId = ((await response.json()) as { executionId: string }).executionId
  let execution: Execution | undefined
  await expect.poll(async () => {
    execution = await request<Execution>(page, token, `/executions/${executionId}`)
    return execution.status
  }, { timeout: 180_000, intervals: [500, 750, 1_000, 2_000] }).toBe(expectedStatus)
  const runs = await request<{ items: NodeRun[] }>(page, token, `/executions/${executionId}/nodes`)
  return { execution: execution!, runs: runs.items }
}

function output(runs: NodeRun[], nodeId: string, port: string) {
  return runs.find((run) => run.nodeId === nodeId)?.output?.[port] ?? []
}

function node(id: string, type: string, name: string, parameters: Record<string, unknown> = {}) {
  return {
    id, key: id, type, typeVersion: 1, name, disabled: false,
    parameters, contextWrites: [], resourceReferences: [], settings: {},
  }
}

function exitNode(outputs: Record<string, unknown> = {}, errorOutputs: Record<string, unknown> = {}) {
  return {
    id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true,
    parameters: { outputs, errorOutputs }, contextWrites: [], resourceReferences: [], settings: {},
  }
}

function edge(id: string, source: string, sourceHandle: string, target: string, targetHandle: string, order = 0) {
  return { id, sourceNodeId: source, sourceHandle, targetNodeId: target, targetHandle, order }
}

const scoreAtLeast = (minimum: number) => ({
  left: {
    kind: 'reference',
    selector: { namespace: 'inputs', run: { kind: 'current' }, item: { kind: 'current' }, path: ['score'] },
    missingPolicy: { kind: 'error' },
  },
  operator: 'gte',
  right: { kind: 'literal', value: minimum },
})

const textTemplate = (text: string) => ({ kind: 'template', segments: [{ kind: 'text', text }] })
const referenceTemplate = (selector: Record<string, unknown>) => ({ kind: 'template', segments: [{ kind: 'reference', selector, missingPolicy: { kind: 'error' } }] })

function loopFailureDefinition(errorMode: 'continue' | 'remove', endpoint: string): Definition {
  return {
    schemaVersion: '8.0',
    start: {
      inputs: {
        type: 'object',
        properties: {
          items: {
            type: 'array',
            items: {
              type: 'object',
              properties: { score: { type: 'number' }, fail: { type: 'boolean' } },
              required: ['score', 'fail'],
              additionalProperties: false,
            },
          },
        },
        required: ['items'],
        additionalProperties: false,
      },
      contexts: {},
    },
    nodes: [
      node('loop', 'loop_over_items', `Loop ${errorMode}`, {
        input: { kind: 'reference', selector: { namespace: 'inputs', run: { kind: 'current' }, item: { kind: 'current' }, path: ['items'] }, missingPolicy: { kind: 'error' } },
        outputSelector: { kind: 'reference', selector: { namespace: 'loop', run: { kind: 'current' }, item: { kind: 'current' }, path: ['item'] }, missingPolicy: { kind: 'error' } },
        parallelism: 2,
        errorMode,
      }),
      {
        ...node('body', 'declarative_http', 'Fail selected rounds', {
          method: 'GET',
          url: textTemplate(endpoint),
          query: [
            { name: 'score', value: referenceTemplate({ namespace: 'loop', run: { kind: 'current' }, item: { kind: 'current' }, path: ['item', 'score'] }) },
            { name: 'fail', value: referenceTemplate({ namespace: 'loop', run: { kind: 'current' }, item: { kind: 'current' }, path: ['item', 'fail'] }) },
          ],
          headers: [],
        }),
        parentId: 'loop',
      },
      exitNode(),
    ],
    connections: [
      edge('start-loop', '__start__', 'main', 'loop', 'main'),
      edge('loop-end', 'loop', 'main', 'exit', 'main'),
    ],
    end: { completion: 'first_return', outputs: {}, error: { outputs: {} } },
    settings: { executionOrder: 'deterministic', activationBudget: 100 },
  }
}

test('M6 local built-ins execute branch, list and error-wiring workflows through Studio', async ({ page }, testInfo) => {
  test.slow()
  const token = await login(page)
  const actor = await request<{ id: string }>(page, token, '/auth/me')
  const suffix = Date.now()

  // Flow 1: if multi-case branching routes the first match and merge appends.
  const branchId = await createWorkflow(page, token, `M6 Branch Merge ${suffix}`)
  await saveDefinition(page, token, branchId, {
    schemaVersion: '8.0',
    start: {
      inputs: { type: 'object', properties: { score: { type: 'number' } }, required: ['score'], additionalProperties: false },
      contexts: {},
    },
    nodes: [
      node('seed', 'set', 'Seed'),
      node('branch', 'if', 'Branch', {
        cases: [{ id: 'high', name: 'IF', conditions: [{ condition: scoreAtLeast(2) }], logicalOp: 'and' }],
      }),
      node('join', 'merge', 'Join', { mode: 'append' }),
      exitNode(),
    ],
    connections: [
      edge('start-seed', '__start__', 'main', 'seed', 'main'),
      edge('seed-branch', 'seed', 'main', 'branch', 'main'),
      edge('high-join', 'branch', 'case:high', 'join', 'main:0'),
      edge('else-join', 'branch', 'else', 'join', 'main:1'),
      edge('join-end', 'join', 'main', 'exit', 'main'),
    ],
    end: { completion: 'first_return', outputs: {}, error: { outputs: {} } },
    settings: { executionOrder: 'deterministic', activationBudget: 100 },
  })
  const branch = await runFromStudio(page, token, branchId, { score: 3 }, 'succeeded')
  expect(output(branch.runs, 'branch', 'case:high')).toHaveLength(1)
  expect(output(branch.runs, 'branch', 'else')).toHaveLength(0)
  expect(output(branch.runs, 'join', 'main')).toHaveLength(1)
  await page.screenshot({ path: testInfo.outputPath('branch-merge-chain.png'), fullPage: true })

  // Flow 2: HTTP returns an embedded array, List produces one {items} value,
  // and Loop processes each selected item with bounded parallelism.
  const listId = await createWorkflow(page, token, `M6 List Operator ${suffix}`)
  const departments = await request<Array<{ id: string; isRoot: boolean }>>(page, token, '/departments')
  const ownerDepartmentId = (departments.find((department) => department.isRoot) ?? departments[0]).id
  const httpSecret = `plan5-http-secret-${suffix}`
  const credential = await request<{ id: string }>(page, token, '/credentials', 'POST', {
    name: `M6 HTTP Credential ${suffix}`,
    credentialType: 'api_key',
    secret: httpSecret,
    ownerDepartmentId,
  })
  await request(page, token, `/workflows/${listId}/resource-authorizations`, 'POST', {
    resourceType: 'credential', resourceId: credential.id, resourceVersionId: null, operation: 'use',
  }, { 'Idempotency-Key': `m6-http-credential-${suffix}` })
  const sandboxName = `M6 HTTP Parse ${suffix}`
  const sandboxes = await request<{ items: Array<{ id: string; name: string }> }>(page, token, `/sandbox-profiles?pageSize=100&search=${encodeURIComponent(sandboxName)}`)
  const sandbox = sandboxes.items.find((item) => item.name === sandboxName) ?? await request<{ id: string; name: string }>(page, token, '/sandbox-profiles', 'POST', {
    name: sandboxName,
    description: 'Parse the Dify-style HTTP body string for the List/Loop E2E',
    ownerDepartmentId,
    runner: 'python',
    imageDigest: sandboxImageDigest(),
    cpuMillis: 500,
    memoryBytes: 536870912,
    pidsLimit: 256,
    diskBytes: 1073741824,
    timeoutSeconds: 120,
    outputLimitBytes: 1048576,
    networkPolicy: { defaultAction: 'deny', egressMode: 'none' },
  })
  await request(page, token, `/workflows/${listId}/resource-authorizations`, 'POST', {
    resourceType: 'sandbox_profile', resourceId: sandbox.id, resourceVersionId: null, operation: 'use',
  }, { 'Idempotency-Key': `m6-http-parse-sandbox-${suffix}` })
  await saveDefinition(page, token, listId, {
    schemaVersion: '8.0',
    start: {
      inputs: { type: 'object', properties: { items: { type: 'array', items: { type: 'object', properties: { score: { type: 'number' } }, required: ['score'], additionalProperties: false } } }, required: ['items'], additionalProperties: false },
      contexts: {},
    },
    nodes: [
      { ...node('http', 'declarative_http', 'Load Items', {
        method: 'POST',
        url: textTemplate(`${echoBaseUrl}/v1/plan5/items`),
        query: [{ name: 'source', value: textTemplate('agentx-plan5') }],
        headers: [{ name: 'x-agentx-test', value: textTemplate('list-loop') }],
        body: { kind: 'object', fields: { items: { kind: 'reference', selector: { namespace: 'inputs', run: { kind: 'current' }, item: { kind: 'current' }, path: ['items'] }, missingPolicy: { kind: 'error' } } } },
        apiKeyPlacement: { in: 'header', name: 'x-plan5-secret' },
      }), resourceReferences: [{ bindingRole: 'credential', resourceType: 'credential', resourceId: credential.id, operation: 'use' }], settings: { timeoutMs: 30_000 } },
      { ...node('parse', 'code', 'Parse HTTP Body', {
        runner: 'python',
        inputs: { kind: 'object', fields: { response_body: { kind: 'reference', selector: { namespace: 'outputs', sourceNodeId: 'http', port: 'main', run: { kind: 'current' }, item: { kind: 'current' }, path: ['body'] }, missingPolicy: { kind: 'error' } } } },
        source: 'import json\n\ndef main(**inputs):\n    payload = json.loads(inputs["response_body"])\n    return {"items": payload["json"]["items"]}',
        outputExample: { items: [{ score: 0 }] },
        networkPolicy: { mode: 'deny', destinations: [] },
      }), resourceReferences: [{ resourceType: 'sandbox_profile', resourceId: sandbox.id, operation: 'use' }] },
      node('top', 'list', 'Top Items', {
        input: { kind: 'reference', selector: { namespace: 'outputs', sourceNodeId: 'parse', port: 'main', run: { kind: 'current' }, item: { kind: 'current' }, path: ['structuredOutput', 'items'] }, missingPolicy: { kind: 'error' } },
        filter: { conditions: [{ condition: { ...scoreAtLeast(0), left: { kind: 'reference', selector: { namespace: 'item', run: { kind: 'current' }, item: { kind: 'current' }, path: ['score'] }, missingPolicy: { kind: 'error' } } } }], logicalOp: 'and' },
        sort: [{ selector: { kind: 'reference', selector: { namespace: 'item', run: { kind: 'current' }, item: { kind: 'current' }, path: ['score'] }, missingPolicy: { kind: 'error' } }, direction: 'desc', nulls: 'last' }],
        takeN: 2,
      }),
      node('loop', 'loop_over_items', 'Loop Items', {
        input: { kind: 'reference', selector: { namespace: 'outputs', sourceNodeId: 'top', port: 'main', run: { kind: 'current' }, item: { kind: 'current' }, path: ['items'] }, missingPolicy: { kind: 'error' } },
        outputSelector: { kind: 'reference', selector: { namespace: 'outputs', sourceNodeId: 'body', port: 'main', run: { kind: 'current' }, item: { kind: 'current' }, path: [] }, missingPolicy: { kind: 'error' } },
        parallelism: 2,
        errorMode: 'terminate',
      }),
      { ...node('body', 'set', 'Mark Processed', { values: { kind: 'object', fields: { processed: { kind: 'literal', value: true } } }, keepOnlySet: false }), parentId: 'loop' },
      node('join', 'merge', 'Merge Loop Result', { mode: 'append' }),
      exitNode(),
    ],
    connections: [
      edge('start-http', '__start__', 'main', 'http', 'main'),
      edge('http-parse', 'http', 'main', 'parse', 'main'),
      edge('parse-top', 'parse', 'main', 'top', 'main'),
      edge('top-loop', 'top', 'main', 'loop', 'main'),
      edge('loop-join', 'loop', 'main', 'join', 'main'),
      edge('join-end', 'join', 'main', 'exit', 'main'),
    ],
    end: { completion: 'first_return', outputs: {}, error: { outputs: {} } },
    settings: { executionOrder: 'deterministic', activationBudget: 100 },
  })
  const listed = await runFromStudio(page, token, listId, { items: [{ score: 5 }, { score: 9 }, { score: 3 }] }, 'succeeded')
  expect(output(listed.runs, 'http', 'main')[0].json.statusCode).toBe(200)
  expect(JSON.stringify(output(listed.runs, 'http', 'main')[0].json)).not.toContain(httpSecret)
  expect(JSON.stringify(output(listed.runs, 'http', 'main')[0].json)).toContain('[REDACTED]')
  expect(output(listed.runs, 'parse', 'main')[0].json.structuredOutput).toMatchObject({ items: [{ score: 5 }, { score: 9 }, { score: 3 }] })
  expect(output(listed.runs, 'top', 'main')).toHaveLength(1)
  expect((output(listed.runs, 'top', 'main')[0].json.items as Array<{ score: number }>)[0].score).toBe(9)
  expect(output(listed.runs, 'loop', 'main')).toHaveLength(1)
  expect(output(listed.runs, 'loop', 'main')[0].json.items).toEqual([
    { score: 9, processed: true },
    { score: 5, processed: true },
  ])
  expect(output(listed.runs, 'join', 'main')[0].json.items).toEqual([
    { score: 9, processed: true },
    { score: 5, processed: true },
  ])
  await page.screenshot({ path: testInfo.outputPath('list-operator-chain.png'), fullPage: true })

  // Flow 2b: failed Loop rounds are materialized according to the selected
  // policy in the real Kubernetes runtime. The middle item omits `score`, so
  // the body reference fails while the surrounding rounds still succeed.
  const failingItems = { items: [{ score: 1, fail: false }, { score: 2, fail: true }, { score: 3, fail: false }] }
  for (const [errorMode, expectedItems] of [
    ['continue', [{ score: 1, fail: false }, null, { score: 3, fail: false }]],
    ['remove', [{ score: 1, fail: false }, { score: 3, fail: false }]],
  ] as const) {
    const workflowId = await createWorkflow(page, token, `M6 Loop ${errorMode} ${suffix}`)
    await saveDefinition(page, token, workflowId, loopFailureDefinition(errorMode, `${echoBaseUrl}/v1/plan5/maybe-fail`))
    const result = await runFromStudio(page, token, workflowId, failingItems, 'succeeded')
    expect(result.runs.filter((run) => run.nodeId === 'body' && run.status === 'succeeded')).toHaveLength(2)
    expect(result.runs.filter((run) => run.nodeId === 'body' && run.status === 'failed')).toHaveLength(1)
    expect(output(result.runs, 'loop', 'main')).toHaveLength(1)
    expect(output(result.runs, 'loop', 'main')[0].json.items).toEqual(expectedItems)
  }
  await page.screenshot({ path: testInfo.outputPath('loop-continue-remove.png'), fullPage: true })

  // Flow 2c: non-text HTTP responses are stored as Runtime Artifacts and the
  // node result carries only safe file metadata.
  const binaryId = await createWorkflow(page, token, `M6 HTTP Binary ${suffix}`)
  await saveDefinition(page, token, binaryId, {
    schemaVersion: '8.0',
    start: { inputs: { type: 'object', properties: {}, additionalProperties: false }, contexts: {} },
    nodes: [
      { ...node('binary', 'declarative_http', 'Download Binary', { method: 'GET', url: textTemplate(`${echoBaseUrl}/v1/plan5/binary`), query: [], headers: [] }), settings: { timeoutMs: 30_000 } },
      exitNode(),
    ],
    connections: [
      edge('start-binary', '__start__', 'main', 'binary', 'main'),
      edge('binary-exit', 'binary', 'main', 'exit', 'main'),
    ],
    end: { completion: 'first_return', outputs: {}, error: { outputs: {} } },
    settings: { executionOrder: 'deterministic', activationBudget: 100 },
  })
  const binary = await runFromStudio(page, token, binaryId, {}, 'succeeded')
  const binaryOutput = output(binary.runs, 'binary', 'main')[0].json as {
    body: unknown
    files: Array<{ artifactId: string; fileName: string; contentType: string; sizeBytes: number; sha256: string }>
  }
  expect(binaryOutput.body).toBe('')
  expect(binaryOutput.files).toEqual([
    expect.objectContaining({
      artifactId: expect.stringMatching(/^[0-9a-f-]{36}$/),
      fileName: 'response.bin',
      contentType: 'application/octet-stream',
      sizeBytes: 8,
      sha256: expect.stringMatching(/^[0-9a-f]{64}$/),
    }),
  ])
  const downloaded = await page.request.get(`/api/v1/executions/${binary.execution.id}/artifacts/${binaryOutput.files[0].artifactId}`, {
    headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() },
  })
  if (!downloaded.ok()) throw new Error(`download HTTP binary Artifact: ${downloaded.status()} ${await downloaded.text()}`)
  expect([...await downloaded.body()]).toEqual([0, 1, 2, 3, 0x7f, 0x80, 0xfe, 0xff])

  // Flow 2d: every supported credential shape is injected into a real HTTP
  // request and then redacted from the reflected response and node output.
  const authId = await createWorkflow(page, token, `M6 HTTP Auth ${suffix}`)
  const authCredentials = [
    { id: 'bearer', credentialType: 'bearer', secret: `bearer-secret-${suffix}`, placement: undefined },
    { id: 'basic', credentialType: 'basic', secret: JSON.stringify({ username: `basic-user-${suffix}`, password: `basic-password-${suffix}` }), placement: undefined },
    { id: 'api_key', credentialType: 'api_key', secret: `api-secret-${suffix}`, placement: { in: 'query', name: 'api_token' } },
    { id: 'custom', credentialType: 'custom_json', secret: JSON.stringify({ headers: { 'x-custom-auth': `custom-header-${suffix}` }, query: { custom_token: `custom-query-${suffix}` } }), placement: undefined },
  ] as const
  const credentialIds = new Map<string, string>()
  for (const credentialSpec of authCredentials) {
    const created = await request<{ id: string }>(page, token, '/credentials', 'POST', {
      name: `M6 HTTP ${credentialSpec.id} ${suffix}`,
      credentialType: credentialSpec.credentialType,
      secret: credentialSpec.secret,
      ownerDepartmentId,
    })
    credentialIds.set(credentialSpec.id, created.id)
    await request(page, token, `/workflows/${authId}/resource-authorizations`, 'POST', {
      resourceType: 'credential', resourceId: created.id, resourceVersionId: null, operation: 'use',
    }, { 'Idempotency-Key': `m6-http-${credentialSpec.id}-${suffix}` })
  }
  const authNodes = authCredentials.map((credentialSpec) => ({
    ...node(credentialSpec.id, 'declarative_http', `HTTP ${credentialSpec.id}`, {
      method: 'GET',
      url: textTemplate(`${echoBaseUrl}/v1/plan5/request`),
      query: [],
      headers: [],
      ...(credentialSpec.placement ? { apiKeyPlacement: credentialSpec.placement } : {}),
    }),
    resourceReferences: [{
      bindingRole: 'credential', resourceType: 'credential', resourceId: credentialIds.get(credentialSpec.id), operation: 'use',
    }],
    settings: { timeoutMs: 30_000 },
  }))
  await saveDefinition(page, token, authId, {
    schemaVersion: '8.0',
    start: { inputs: { type: 'object', properties: {}, additionalProperties: false }, contexts: {} },
    nodes: [...authNodes, exitNode()],
    connections: [
      edge('start-bearer', '__start__', 'main', 'bearer', 'main'),
      edge('bearer-basic', 'bearer', 'main', 'basic', 'main'),
      edge('basic-api', 'basic', 'main', 'api_key', 'main'),
      edge('api-custom', 'api_key', 'main', 'custom', 'main'),
      edge('custom-exit', 'custom', 'main', 'exit', 'main'),
    ],
    end: { completion: 'first_return', outputs: {}, error: { outputs: {} } },
    settings: { executionOrder: 'deterministic', activationBudget: 100 },
  })
  const authenticated = await runFromStudio(page, token, authId, {}, 'succeeded')
  for (const credentialSpec of authCredentials) {
    const response = output(authenticated.runs, credentialSpec.id, 'main')[0].json
    expect(response.statusCode).toBe(200)
    expect(JSON.stringify(response)).toContain('[REDACTED]')
  }
  const redactedAuthOutput = JSON.stringify(authenticated.runs)
  for (const secret of [
    `bearer-secret-${suffix}`,
    `basic-user-${suffix}`,
    `basic-password-${suffix}`,
    `api-secret-${suffix}`,
    `custom-header-${suffix}`,
    `custom-query-${suffix}`,
  ]) expect(redactedAuthOutput).not.toContain(secret)

  // Flow 3: NodeSettings.timeoutMs interrupts a real HTTPS request and the
  // wired error edge routes the failure to the Exit error port
  // ("wiring is the policy") and the execution ends failed.
  const errorId = await createWorkflow(page, token, `M6 Error Wiring ${suffix}`)
  await saveDefinition(page, token, errorId, {
    schemaVersion: '8.0',
    start: { inputs: { type: 'object', properties: {}, additionalProperties: false }, contexts: {} },
    nodes: [
      { ...node('unreachable', 'declarative_http', 'Timed Request', { method: 'GET', url: textTemplate(`${echoBaseUrl}/v1/plan5/delay`), query: [], headers: [] }), settings: { timeoutMs: 100 } },
      exitNode(),
    ],
    connections: [
      edge('start-http', '__start__', 'main', 'unreachable', 'main'),
      edge('http-end', 'unreachable', 'main', 'exit', 'main'),
      edge('http-error-end', 'unreachable', 'error', 'exit', 'error'),
    ],
    end: { completion: 'first_return', outputs: {}, error: { outputs: {} } },
    settings: { executionOrder: 'deterministic', activationBudget: 100 },
  })
  const failed = await runFromStudio(page, token, errorId, {}, 'failed')
  const httpRun = failed.runs.find((run) => run.nodeId === 'unreachable')
  expect(httpRun?.status).toBe('failed')
  expect(httpRun?.errorCode).toBeTruthy()
  await page.screenshot({ path: testInfo.outputPath('error-wiring-chain.png'), fullPage: true })

  // Flow 4: first_return cancels a pending Approval task on the losing branch.
  const cleanupId = await createWorkflow(page, token, `M6 First Return Cleanup ${suffix}`)
  const customExit = (id: string, key: string) => ({ ...exitNode(), id, key, protected: id === 'fast_exit' })
  await saveDefinition(page, token, cleanupId, {
    schemaVersion: '8.0',
    start: { inputs: { type: 'object', properties: {}, additionalProperties: false }, contexts: {} },
    nodes: [
      node('approval', 'approval', 'Pending Approval', { candidateUserId: actor.id, title: textTemplate('Must be cancelled') }),
      node('fast', 'set', 'Fast Result', { values: { kind: 'object', fields: { done: { kind: 'literal', value: true } } }, keepOnlySet: true }),
      customExit('fast_exit', 'fast_exit'),
      customExit('approval_exit', 'approval_exit'),
    ],
    connections: [
      edge('start-approval', '__start__', 'main', 'approval', 'main', 0),
      edge('start-fast', '__start__', 'main', 'fast', 'main', 1),
      edge('approval-exit', 'approval', 'decision:approved', 'approval_exit', 'main'),
      edge('fast-exit', 'fast', 'main', 'fast_exit', 'main'),
    ],
    end: { completion: 'first_return', outputs: {}, error: { outputs: {} } },
    settings: { executionOrder: 'parallel', activationBudget: 100 },
  })
  const cleaned = await runFromStudio(page, token, cleanupId, {}, 'succeeded')
  await expect.poll(async () => {
    const approvals = await request<{ items: Array<{ executionId: string; status: string }> }>(page, token, '/approvals?pageSize=100')
    return approvals.items.find((approval) => approval.executionId === cleaned.execution.id)?.status
  }, { timeout: 60_000, intervals: [500, 1_000, 2_000] }).toBe('cancelled')
})
