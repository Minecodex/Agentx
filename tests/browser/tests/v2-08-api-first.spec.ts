import { fixtureDraftId } from '../draft-fixtures'
import { expect, type APIResponse, type Page, test } from '@playwright/test'
import { writeFile } from 'node:fs/promises'

import { publishCompatibleChatMapping } from './playground-helpers'

const password = 'agentx-e2e-admin-password'
const gatewayBase = process.env.AGENTX_E2E_RUNTIME_URL ?? ''

type Draft = { revision: number; editorDocument: Record<string, unknown> }
type Environment = { id: string; code: string }
type Deployment = { id: string; status: string; publishAttemptId?: string }
type Invocation = { id: string; executionId?: string; status: string; outputs?: Record<string, unknown> }

const reference = (namespace: 'inputs' | 'outputs', path: string[], sourceNodeId?: string) => ({
  kind: 'reference',
  selector: { namespace, sourceNodeId, port: sourceNodeId ? 'main' : undefined, run: { kind: 'current' }, item: { kind: 'current' }, path },
  missingPolicy: { kind: 'error' },
})

async function expectResponse(response: APIResponse, label: string) {
  if (!response.ok()) throw new Error(`${label}: ${response.status()} ${await response.text()}`)
}

async function request<T>(page: Page, token: string, path: string, method = 'GET', body?: unknown, gateway = false, headers: Record<string, string> = {}) {
  const response = await page.request.fetch(`${gateway ? `${gatewayBase}/gateway/v1` : '/api/v1'}${path}`, {
    method,
    data: body,
    headers: { Authorization: `Bearer ${token}`, ...headers },
  })
  await expectResponse(response, `${method} ${path}`)
  const text = await response.text()
  return (text ? JSON.parse(text) : undefined) as T
}

async function login(page: Page) {
  const status = await page.request.get('/api/v1/bootstrap/status')
  await expectResponse(status, 'bootstrap status')
  if (((await status.json()) as { required: boolean }).required) {
    const bootstrap = await page.request.post('/api/v1/bootstrap', {
      data: {
        companyName: 'Agentx E2E', adminUsername: 'admin',
        adminDisplayName: 'E2E Admin', password, locale: 'zh-CN', timezone: 'Asia/Shanghai',
      },
    })
    await expectResponse(bootstrap, 'bootstrap')
    return ((await bootstrap.json()) as { accessToken: string }).accessToken
  }
  const response = await page.request.post('/api/v1/auth/login', { data: { username: 'admin', password } })
  await expectResponse(response, 'login')
  return ((await response.json()) as { accessToken: string }).accessToken
}

async function waitInvocation(page: Page, token: string, id: string) {
  let value: Invocation | undefined
  await expect.poll(async () => {
    value = await request<Invocation>(page, token, `/invocations/${id}`, 'GET', undefined, true)
    return value.status
  }, { timeout: 180_000, intervals: [250, 500, 1_000, 2_000] }).toMatch(/completed|failed|cancelled/)
  return value!
}

test('V2-08 API-first empty-domain closure creates product facts without SQL fixtures', async ({ page }) => {
  const token = await login(page)
  const suffix = Date.now()
  const workflow = await request<{ id: string }>(page, token, '/workflows', 'POST', {
    name: `V2-08 API First ${suffix}`, description: 'V2-08 empty-domain closure', visibility: 'company',
  })
  const draft = await request<Draft>(page, token, `/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`)
  const definition = {
    schemaVersion: '8.0',
    start: { inputs: { type: 'object', properties: { message: { type: 'string' } }, required: ['message'], additionalProperties: true }, contexts: {} },
    nodes: [
      ...Array.from({ length: 4 }, (_, index) => ({
        id: `step-${index + 1}`, key: `step_${index + 1}`, type: 'set', typeVersion: 1,
        name: `Step ${index + 1}`, disabled: false,
        parameters: { values: { kind: 'object', fields: { message: index === 0 ? reference('inputs', ['message']) : reference('outputs', ['message'], `step-${index}`) } }, keepOnlySet: true },
        contextWrites: [], resourceReferences: [], settings: {},
      })),
      { id: 'exit', key: 'exit', type: 'exit', typeVersion: 1, name: 'End', disabled: false, protected: true, parameters: { outputs: { message: reference('outputs', ['message'], 'step-4') }, errorOutputs: {} }, contextWrites: [], resourceReferences: [], settings: {} },
    ],
    connections: [
      { id: 'start-step-1', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'step-1', targetHandle: 'main', order: 0 },
      { id: 'step-1-step-2', sourceNodeId: 'step-1', sourceHandle: 'main', targetNodeId: 'step-2', targetHandle: 'main', order: 0 },
      { id: 'step-2-step-3', sourceNodeId: 'step-2', sourceHandle: 'main', targetNodeId: 'step-3', targetHandle: 'main', order: 0 },
      { id: 'step-3-step-4', sourceNodeId: 'step-3', sourceHandle: 'main', targetNodeId: 'step-4', targetHandle: 'main', order: 0 },
      { id: 'step-4-end', sourceNodeId: 'step-4', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 },
    ],
    end: { outputs: { message: { schema: { type: 'string' }, required: true, sensitive: false } } },
    settings: { activationBudget: 8, executionOrder: 'deterministic' },
  }
  const saved = await request<{ revision: number }>(page, token, `/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`, 'PUT', {
    expectedRevision: draft.revision, definition,
    editorDocument: {
      ...draft.editorDocument,
      nodeLayouts: Array.from({ length: 4 }, (_, index) => ({ nodeId: `step-${index + 1}`, x: 120 + index * 240, y: 180 })),
      edges: ['start-step-1', 'step-1-step-2', 'step-2-step-3', 'step-3-step-4', 'step-4-end'].map(edgeId => ({ edgeId })),
    },
  })
  const version = await request<{ id: string }>(page, token, `/workflows/${workflow.id}/versions`, 'POST', { draftId: await fixtureDraftId(page, workflow.id),  draftRevision: saved.revision })
  const environment = (await request<Environment[]>(page, token, '/environments')).find((item) => item.code === 'development')
  expect(environment).toBeTruthy()
  await request(page, token, `/workflows/${workflow.id}/deployments`, 'POST', { workflowVersionId: version.id, environmentId: environment!.id })

  const application = await request<{ id: string; slug: string }>(page, token, '/applications', 'POST', {
    workflowId: workflow.id, name: `V2-08 Application ${suffix}`, slug: `v2-08-${suffix}`,
    description: 'API-first runtime closure', visibility: 'company',
  })
  const apiKey = await request<{ id: string; secret: string }>(page, token, `/applications/${application.id}/api-keys`, 'POST', { name: 'V2-08 E2E' })
  const deployment = await request<Deployment>(page, token, `/applications/${application.id}/deployments`, 'POST', {
    workflowVersionId: version.id, environmentId: environment!.id, sessionVersionPolicy: 'pinned',
  })
  await expect.poll(async () => {
    const values = await request<Deployment[]>(page, token, `/applications/${application.id}/deployments`)
    const status = values.find((item) => item.id === deployment.id)?.status
    if (status === 'rejected') throw new Error(`application deployment ${deployment.id} was rejected`)
    return status
  }, { timeout: 180_000, intervals: [500, 1_000, 2_000] }).toBe('active')
  await publishCompatibleChatMapping(page, token, application.id, deployment.id)

  const invoke = async (key: string) => page.request.post(`${gatewayBase}/gateway/v1/applications/${application.slug}/invocations`, {
    data: { input: { message: 'closed' }, responseMode: 'async' },
    headers: { Authorization: `Bearer ${apiKey.secret}`, 'Idempotency-Key': key },
  })
  let started: APIResponse | undefined
  await expect.poll(async () => {
    started = await invoke(`v2-08-start-${suffix}`)
    return started.status()
  }, { timeout: 60_000, intervals: [500, 1_000, 2_000] }).toBe(202)
  const invocation = await started!.json() as Invocation
  const completed = await waitInvocation(page, token, invocation.id)
  expect(completed).toMatchObject({ status: 'completed', outputs: { message: 'closed' } })

  const session = await request<{ id: string }>(page, apiKey.secret, `/applications/${application.slug}/sessions`, 'POST', { title: 'V2-08 session' }, true, { 'Idempotency-Key': `v2-08-session-${suffix}` })
  const message = await request<Invocation>(page, apiKey.secret, `/sessions/${session.id}/messages`, 'POST', {
    parts: [{ partType: 'text', content: 'session', artifactId: null }],
  }, true, { 'Idempotency-Key': `v2-08-message-${suffix}` })
  expect((await waitInvocation(page, token, message.id)).status).toBe('completed')
  await expect.poll(async () => {
    const messages = await request<Array<{ role: string }>>(page, apiKey.secret, `/sessions/${session.id}/messages`, 'GET', undefined, true)
    return messages.map(item => item.role)
  }, { timeout: 10_000, intervals: [100, 250, 500] }).toContain('assistant')

  const dataset = await request<{ id: string }>(page, token, '/datasets', 'POST', { name: `V2-08 Dataset ${suffix}`, description: 'API-first', visibility: 'company' })
  await request(page, token, `/datasets/${dataset.id}/cases`, 'POST', {
    expectedRevision: 0, caseKey: 'case-1', name: 'Case 1', input: { message: 'evaluation' },
    expectedOutput: { message: 'evaluation' }, context: null, tags: ['v2-08'], evaluatorOverride: null,
  })
  const datasetVersion = await request<{ id: string }>(page, token, `/datasets/${dataset.id}/versions`, 'POST')
  const profile = await request<{ versionId: string }>(page, token, '/evaluation-profiles', 'POST', {
    name: `V2-08 Exact ${suffix}`, description: 'API-first', visibility: 'company', aggregation: 'all', passThreshold: '1',
    rules: [{ key: 'exact', name: 'Exact', evaluatorType: 'exact', configuration: {}, weight: '1', required: true }],
  })
  const evaluation = await request<{ id: string }>(page, token, '/evaluations', 'POST', {
    name: `V2-08 Evaluation ${suffix}`, workflowVersionId: version.id, datasetVersionId: datasetVersion.id,
    evaluationProfileVersionId: profile.versionId, visibility: 'company', parameters: {},
  })
  await request(page, token, `/evaluations/${evaluation.id}/start`, 'POST')
  await expect.poll(async () => {
    const response = await page.request.get(`/api/v1/evaluations/${evaluation.id}/report`, { headers: { Authorization: `Bearer ${token}` } })
    return response.status()
  }, { timeout: 180_000, intervals: [500, 1_000, 2_000] }).toBe(200)

  const contextPath = process.env.AGENTX_V2_08_CONTEXT_OUTPUT
  if (!contextPath) throw new Error('AGENTX_V2_08_CONTEXT_OUTPUT is required')
  await writeFile(contextPath, JSON.stringify({
    applicationId: application.id, applicationSlug: application.slug, apiKey: apiKey.secret,
    sessionId: session.id, invocationId: completed.id, executionId: completed.executionId,
    workflowId: workflow.id, workflowVersionId: version.id, evaluationId: evaluation.id,
  }), { encoding: 'utf8', mode: 0o600 })
})
