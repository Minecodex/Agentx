import { fixtureDraftId, fixtureDraftIdKnown } from '../draft-fixtures'
import { connect, login } from '../support/canvas-plugin'
import { showEdgeToolbar } from './workflow-studio-editor-helpers'
import { expect, type Locator, type Page, test } from '@playwright/test'
import { readFile, writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'

const password = 'agentx-e2e-admin-password'
const gatewayBase = process.env.AGENTX_E2E_RUNTIME_URL ?? ''

async function api<T>(page: Page, token: string, path: string): Promise<T> {
  const response = await page.request.get(`/api/v1${path}`, { headers: { Authorization: `Bearer ${token}` } })
  if (!response.ok()) throw new Error(`${path}: ${response.status()} ${await response.text()}`)
  return response.json() as Promise<T>
}

async function waitInvocation(page: Page, apiKey: string, id: string) {
  let value: { status: string; outputs?: Record<string, unknown> } | undefined
  await expect.poll(async () => {
    const response = await page.request.get(`${gatewayBase}/gateway/v1/invocations/${id}`, { headers: { Authorization: `Bearer ${apiKey}` } })
    if (response.status() === 404) return 'pending'
    if (!response.ok()) throw new Error(`/invocations/${id}: ${response.status()} ${await response.text()}`)
    const latest = await response.json() as { status: string; outputs?: Record<string, unknown> }
    value = latest
    return latest.status
  }, { timeout: 180_000, intervals: [250, 500, 1_000, 2_000] }).toMatch(/completed|failed|cancelled/)
  return value!
}

async function chooseReference(page: Page, field: Locator, labels: string[]) {
  await field.getByRole('button', { name: /选择变量|Select variable|Insert variable/ }).click()
  const picker = page.getByTestId('reference-picker')
  await picker.getByRole('button', { name: /输出|Outputs/ }).click()
  const tree = picker.locator(':scope > div').nth(1)
  for (const [index, label] of labels.entries()) {
    const row = tree.getByRole('button', { name: new RegExp(label, 'i') }).first()
    if (index === 0 && await row.count() === 0) continue
    await expect(row).toBeVisible()
    const toggle = row.locator('[data-tree-toggle]')
    if (index < labels.length - 1 && await toggle.count()) await toggle.click()
    else await row.click()
  }
  await expect(picker).toBeHidden()
}

async function configureExitReference(page: Page) {
  await page.getByTestId('exit-node-exit').click()
  const panel = page.getByTestId('exit-panel')
  await panel.getByRole('button', { name: /添加字段|Add field/ }).first().click()
  const dialog = page.getByRole('dialog', { name: /输出字段|Output field/ })
  await dialog.getByLabel(/输出名称|Output name/).fill('mapped')
  await dialog.getByLabel(/类型|Type/).click()
  await page.getByRole('option', { name: /数字|Number/ }).click()
  await dialog.getByRole('button', { name: /保存|Save/ }).click()
  const mapping = panel.getByTestId('exit-mapping-mapped')
  await chooseReference(page, mapping, ['acme_json_mapper', 'metadata', 'current', 'mapped'])
  await panel.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
}

test('imports a TypeScript canvas plugin and edits its static platform controls', async ({ page }, testInfo) => {
  const token = await login(page)
  const packagePath = resolve(import.meta.dirname, '../../../src/plugins/templates/canvas-plugin/dist/acme-json-mapper.agentx-plugin')
  const packageBytes = await readFile(packagePath)
  const builtins = await api<{ items: Array<{ packageId: string; sourceType: string }> }>(page, token, '/canvas-plugins?pageSize=100')
  expect(builtins.items).toEqual(expect.arrayContaining([
    expect.objectContaining({ packageId: 'agentx/core', sourceType: 'builtin' }),
    expect.objectContaining({ packageId: 'agentx/data', sourceType: 'builtin' }),
    expect.objectContaining({ packageId: 'agentx/http', sourceType: 'builtin' }),
  ]))
  const template = await page.request.get('/api/v1/canvas-plugin-sdk/template', { headers: { Authorization: `Bearer ${token}` } })
  expect(template.ok(), await template.text()).toBeTruthy()
  expect((await template.body()).length).toBeGreaterThan(5_000)
  const invalid = await page.request.post('/api/v1/canvas-plugin-imports', {
    headers: { Authorization: `Bearer ${token}` },
    multipart: { file: { name: 'invalid.agentx-plugin', mimeType: 'application/zip', buffer: Buffer.from('not a zip') } },
  })
  expect(invalid.status()).toBe(201)
  const invalidImport = await invalid.json() as { id: string; status: string; issues: unknown[] }
  expect(invalidImport.status).toBe('failed')
  expect(invalidImport.issues).not.toHaveLength(0)
  expect((await api<{ status: string }>(page, token, `/canvas-plugin-imports/${invalidImport.id}`)).status).toBe('failed')
  expect((await page.request.delete(`/api/v1/canvas-plugin-imports/${invalidImport.id}`, { headers: { Authorization: `Bearer ${token}` } })).status()).toBe(204)
  const invalidRetry = await page.request.post('/api/v1/canvas-plugin-imports', { headers: { Authorization: `Bearer ${token}` }, multipart: { file: { name: 'invalid-retry.agentx-plugin', mimeType: 'application/zip', buffer: Buffer.from('not a zip') } } })
  const invalidRetried = await invalidRetry.json() as { id: string; status: string }
  expect(invalidRetried.status).toBe('failed')
  expect((await page.request.delete(`/api/v1/canvas-plugin-imports/${invalidRetried.id}`, { headers: { Authorization: `Bearer ${token}` } })).status()).toBe(204)
  await page.getByRole('link', { name: '画布插件', exact: true }).click()
  await expect(page).toHaveURL(/\/canvas-plugins$/)
  if (!builtins.items.some((item) => item.packageId === 'acme/json-mapper')) {
    await page.getByRole('button', { name: '导入插件' }).click()
    const dialog = page.getByRole('dialog', { name: '导入画布插件' })
    await dialog.locator('input[type=file]').setInputFiles(packagePath)
    await expect(dialog.getByText('acme/json-mapper@1.0.0')).toBeVisible()
    await dialog.getByRole('button', { name: '确认导入' }).click()
    await expect(dialog).toBeHidden()
  }
  await page.getByRole('searchbox', { name: '搜索插件' }).fill('acme/json-mapper')
  await expect(page.getByText('JSON Mapper', { exact: true }).first()).toBeVisible()
  const managementShot = testInfo.outputPath('canvas-plugins-light-zh.png')
  await page.screenshot({ path: managementShot, fullPage: true })
  await testInfo.attach('canvas-plugins-light-zh.png', { path: managementShot, contentType: 'image/png' })
  const resolved = await page.request.post('/api/v1/node-definitions/acme.json_mapper/versions/1/resolve', {
    headers: { Authorization: `Bearer ${token}` }, data: { configuration: { label: 'customer' }, upstreamContracts: {} },
  })
  expect(resolved.ok(), await resolved.text()).toBeTruthy()
  expect((await resolved.json()).status).toBe('complete')
  const provider = await page.request.get('/api/v1/node-definitions/acme.json_mapper/versions/1/providers/labels?search=cus&limit=10', { headers: { Authorization: `Bearer ${token}` } })
  expect(provider.ok(), await provider.text()).toBeTruthy()
  expect((await provider.json()).items).toEqual([{ value: 'customer', label: 'Customer', description: null }])
  const firstProviderPage = await api<{ items: Array<{ value: string }>; nextCursor?: string }>(page, token, '/node-definitions/acme.json_mapper/versions/1/providers/labels?limit=1')
  expect(firstProviderPage).toMatchObject({ items: [{ value: 'customer' }], nextCursor: '1' })
  expect((await api<{ items: Array<{ value: string }> }>(page, token, `/node-definitions/acme.json_mapper/versions/1/providers/labels?limit=1&cursor=${firstProviderPage.nextCursor}`)).items[0].value).toBe('order')
  const missingModelProvider = await page.request.get(`/api/v1/node-definitions/acme.json_mapper/versions/1/providers/labels?parameters=${encodeURIComponent(JSON.stringify({ hostMode: 'missing-model' }))}`, { headers: { Authorization: `Bearer ${token}` } })
  expect(missingModelProvider.status()).toBe(422)
  expect((await missingModelProvider.json()).code).toBe('PLUGIN_DESIGN_OPERATION_FAILED')
  const missingCredentialReference = [{ bindingRole: 'credential', resourceType: 'credential', resourceId: crypto.randomUUID(), operation: 'use' }]
  const missingCredentialProvider = await page.request.get(`/api/v1/node-definitions/acme.json_mapper/versions/1/providers/labels?resourceReferences=${encodeURIComponent(JSON.stringify(missingCredentialReference))}`, { headers: { Authorization: `Bearer ${token}` } })
  expect(missingCredentialProvider.status()).toBe(422)

  const validationWorkflowResponse = await page.request.post('/api/v1/workflows', { headers: { Authorization: `Bearer ${token}` }, data: { name: `Plugin validation ${Date.now()}`, visibility: 'company' } })
  const validationWorkflow = await validationWorkflowResponse.json() as { id: string }
  const validationDraft = await api<{ revision: number; definition: { nodes: Array<Record<string, unknown>>; [key: string]: unknown }; editorDocument: { nodeLayouts?: unknown[]; [key: string]: unknown } }>(page, token, `/workflows/${validationWorkflow.id}/drafts/${await fixtureDraftId(page, validationWorkflow.id)}`)
  const incompleteNode = { id: 'dynamic', key: 'dynamic', type: 'acme.json_mapper', typeVersion: 1, name: 'Dynamic', disabled: false, protected: false, parameters: { label: '', includeMetadata: false, largeTrace: false }, contextWrites: [], resourceReferences: [], settings: {} }
  const validationDefinition = { ...validationDraft.definition, nodes: [incompleteNode, ...validationDraft.definition.nodes], connections: [{ id: 'dynamic-start', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'dynamic', targetHandle: 'main', order: 0 }, { id: 'dynamic-end', sourceNodeId: 'dynamic', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 }] }
  const validationEditor = { ...validationDraft.editorDocument, nodeLayouts: [{ nodeId: 'dynamic', x: 320, y: 200 }, ...(validationDraft.editorDocument.nodeLayouts ?? [])], edges: [{ edgeId: 'dynamic-start' }, { edgeId: 'dynamic-end' }] }
  const incompleteSave = await page.request.put(`/api/v1/workflows/${validationWorkflow.id}/drafts/${await fixtureDraftId(page, validationWorkflow.id)}`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': crypto.randomUUID() }, data: { expectedRevision: validationDraft.revision, definition: validationDefinition, editorDocument: validationEditor } })
  expect(incompleteSave.ok(), await incompleteSave.text()).toBeTruthy()
  const incompleteRevision = ((await incompleteSave.json()) as { revision: number }).revision
  const blockedVersion = await page.request.post(`/api/v1/workflows/${validationWorkflow.id}/versions`, { headers: { Authorization: `Bearer ${token}` }, data: { draftId: await fixtureDraftId(page, validationWorkflow.id),  draftRevision: incompleteRevision } })
  expect(blockedVersion.status()).toBe(422)
  expect((await blockedVersion.json()).code).toBe('PLUGIN_DEFINITION_NOT_READY')
  incompleteNode.parameters.label = '__invalid__'
  const invalidSave = await page.request.put(`/api/v1/workflows/${validationWorkflow.id}/drafts/${await fixtureDraftId(page, validationWorkflow.id)}`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': crypto.randomUUID() }, data: { expectedRevision: incompleteRevision, definition: validationDefinition, editorDocument: validationEditor } })
  expect(invalidSave.status()).toBe(422)
  expect((await api<{ revision: number }>(page, token, `/workflows/${validationWorkflow.id}/drafts/${await fixtureDraftId(page, validationWorkflow.id)}`)).revision).toBe(incompleteRevision)

  const response = await page.request.post('/api/v1/workflows', {
    headers: { Authorization: `Bearer ${token}` },
    data: { name: `Plugin E2E ${Date.now()}`, description: 'Canvas plugin E2E', visibility: 'company' },
  })
  expect(response.ok(), await response.text()).toBeTruthy()
  const workflow = (await response.json()) as { id: string }
  await page.goto(`/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}/editor`)
  const draft = await api<{ definition: { connections: Array<{ id: string }> } }>(page, token, `/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`)
  const initialEdge = page.locator(`.react-flow__edge[data-testid="rf__edge-${draft.definition.connections[0].id}"]`)
  const edgeToolbar = page.locator(`.studio-edge-toolbar[data-edge-id="${draft.definition.connections[0].id}"]`)
  await showEdgeToolbar(initialEdge, edgeToolbar)
  await edgeToolbar.getByRole('button', { name: '删除连线' }).click()
  await expect(page.locator('.react-flow__edge')).toHaveCount(0)
  await page.getByRole('textbox', { name: '搜索节点' }).fill('JSON Mapper')
  await page.getByTestId('palette-action-acme.json_mapper').first().click()
  const inspector = page.getByTestId('node-details-view')
  await expect(inspector.getByLabel('Label', { exact: true })).toBeVisible()
  await inspector.getByLabel('Label').fill('customer')
  await expect(inspector.getByLabel('Search provider')).toHaveCount(0)
  await inspector.getByLabel('Metadata output').check()
  const pluginNode = page.locator('.react-flow__node-manifest').filter({ hasText: 'JSON Mapper' })
  await expect(pluginNode).toBeVisible()
  await expect(pluginNode.locator('.react-flow__handle.source[data-handleid="metadata"]')).toBeVisible()
  await page.getByRole('button', { name: '撤销' }).click()
  await expect(pluginNode.locator('.react-flow__handle.source[data-handleid="metadata"]')).toHaveCount(0)
  await page.getByRole('button', { name: '重做' }).click()
  await expect(pluginNode.locator('.react-flow__handle.source[data-handleid="metadata"]')).toBeVisible()
  await pluginNode.click()
  await expect(inspector.getByLabel('Label', { exact: true })).toBeVisible()
  await inspector.getByLabel('Large trace artifact').check()
  await inspector.getByRole('button', { name: '关闭', exact: true }).first().click()
  await page.getByRole('button', { name: '适应画布', exact: true }).click({ force: true })
  await connect(page, page.getByTestId('workflow-start'), 'main', pluginNode, 'main')
  await connect(page, pluginNode, 'main', page.getByTestId('exit-node-exit'), 'main')
  await configureExitReference(page)
  const save = page.locator('header').getByRole('button', { name: '保存', exact: true })
  let savedRevision = 0
  if (await save.isEnabled()) {
    const saved = page.waitForResponse((value) => value.url().endsWith(`/api/v1/workflows/${workflow.id}/drafts/${fixtureDraftIdKnown(workflow.id)}`) && value.request().method() === 'PUT')
    await save.click()
    const savedResponse = await saved
    expect(savedResponse.ok()).toBeTruthy()
    savedRevision = ((await savedResponse.json()) as { revision: number }).revision
  }
  await expect(page.locator('header').getByText(/修订号 \d+ · 已保存/)).toBeVisible()
  if (!savedRevision) savedRevision = (await api<{ revision: number }>(page, token, `/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`)).revision
  const workflowVersionResponse = await page.request.post(`/api/v1/workflows/${workflow.id}/versions`, { headers: { Authorization: `Bearer ${token}` }, data: { draftId: await fixtureDraftId(page, workflow.id),  draftRevision: savedRevision } })
  expect(workflowVersionResponse.ok(), await workflowVersionResponse.text()).toBeTruthy()
  const workflowVersion = await workflowVersionResponse.json() as { id: string }
  const environment = (await api<Array<{ id: string; code: string }>>(page, token, '/environments')).find((item) => item.code === 'development')!
  const workflowDeployment = await page.request.post(`/api/v1/workflows/${workflow.id}/deployments`, { headers: { Authorization: `Bearer ${token}` }, data: { workflowVersionId: workflowVersion.id, environmentId: environment.id } })
  expect(workflowDeployment.status()).toBe(201)
  const suffix = Date.now().toString()
  const applicationResponse = await page.request.post('/api/v1/applications', { headers: { Authorization: `Bearer ${token}` }, data: { workflowId: workflow.id, name: `Plugin Application ${suffix}`, slug: `plugin-${suffix}`, description: 'Plugin production E2E', visibility: 'company' } })
  expect(applicationResponse.status()).toBe(201)
  const application = await applicationResponse.json() as { id: string; slug: string }
  const apiKeyResponse = await page.request.post(`/api/v1/applications/${application.id}/api-keys`, { headers: { Authorization: `Bearer ${token}` }, data: { name: 'Plugin E2E' } })
  expect(apiKeyResponse.status()).toBe(201)
  const apiKey = await apiKeyResponse.json() as { secret: string }
  const applicationDeploymentResponse = await page.request.post(`/api/v1/applications/${application.id}/deployments`, { headers: { Authorization: `Bearer ${token}` }, data: { workflowVersionId: workflowVersion.id, environmentId: environment.id, sessionVersionPolicy: 'pinned' } })
  expect(applicationDeploymentResponse.status()).toBe(202)
  const applicationDeployment = await applicationDeploymentResponse.json() as { id: string }
  await expect.poll(async () => (await api<Array<{ id: string; status: string }>>(page, token, `/applications/${application.id}/deployments`)).find((item) => item.id === applicationDeployment.id)?.status, { timeout: 180_000, intervals: [500, 1_000, 2_000] }).toBe('active')
  const startProduction = async (key: string) => {
    const response = await page.request.post(`${gatewayBase}/gateway/v1/applications/${application.slug}/invocations`, { headers: { Authorization: `Bearer ${apiKey.secret}`, 'Idempotency-Key': key }, data: { input: {}, responseMode: 'async' } })
    expect(response.status(), await response.text()).toBe(202)
    return await response.json() as { id: string; status: string }
  }
  const invokeProduction = async (key: string) => waitInvocation(page, apiKey.secret, (await startProduction(key)).id)
  expect((await invokeProduction(`plugin-before-disable-${suffix}`)).outputs?.mapped).toBe(1)
  const executionResponse = page.waitForResponse((value) => value.url().includes('/debug-executions') && value.request().method() === 'POST')
  await page.locator('header').getByRole('button', { name: '运行', exact: true }).click()
  const runDialog = page.getByRole('dialog', { name: /运行工作流|调试输入/ })
  if (await runDialog.isVisible().catch(() => false)) await runDialog.getByRole('button', { name: /开始运行|运行/ }).click()
  const accepted = await executionResponse; expect(accepted.status()).toBe(202)
  const executionId = ((await accepted.json()) as { executionId: string }).executionId
  await expect.poll(async () => (await api<{ spans: Array<{ spanKind: string }> }>(page, token, `/executions/${executionId}/trace?limit=100`)).spans.some((span) => span.spanKind === 'plugin_operation'), { timeout: 30_000, intervals: [50, 100, 200] }).toBeTruthy()
  await expect.poll(async () => (await api<{ status: string }>(page, token, `/executions/${executionId}`)).status, { timeout: 120_000 }).toBe('succeeded')
  const completed = await api<{ output?: { mapped?: number } }>(page, token, `/executions/${executionId}`)
  expect(completed.output?.mapped).toBe(1)
  const nodes = await api<{ items: Array<{ nodeType: string; status: string }> }>(page, token, `/executions/${executionId}/nodes`)
  expect(nodes.items).toContainEqual(expect.objectContaining({ nodeType: 'acme.json_mapper', status: 'succeeded' }))
  // Executable trace renderers belong to the user-opened execution detail,
  // outside the editor's static-only binding.
  await page.goto(`/executions/${executionId}`)
  const runtimeRail = page.getByTestId('trace-workspace')
  await runtimeRail.getByRole('tab', { name: '高级瀑布' }).click()
  const pluginSpan = runtimeRail.getByRole('row').filter({ hasText: 'Map items' }).first()
  await expect(pluginSpan).toBeVisible({ timeout: 30_000 }); await pluginSpan.click()
  await runtimeRail.getByRole('tab', { name: '插件业务内容' }).click()
  await expect(runtimeRail.getByText('Mapped items: 0')).toBeVisible()
  await expect(runtimeRail.getByText('Mapped items: 1')).toBeVisible()
  await expect(runtimeRail.getByRole('columnheader', { name: 'label' })).toBeVisible()
  await expect(runtimeRail.getByRole('cell', { name: 'customer' })).toBeVisible()
  const trace = await api<{ spans: Array<{ spanId: string; parentSpanId?: string | null; spanName: string; spanKind: string }> }>(page, token, `/executions/${executionId}/trace?limit=100`)
  const outerSpan = trace.spans.find((span) => span.spanKind === 'plugin_operation' && span.spanName === 'Map items')!
  expect(trace.spans.find((span) => span.spanName === 'Prepare mapping')?.parentSpanId).toBe(outerSpan.spanId)
  const outerDetail = await api<{ contents: Array<{ kind: string; contentRef?: string | null; preview?: unknown }> }>(page, token, `/executions/${executionId}/trace/spans/${outerSpan.spanId}`)
  expect(outerDetail.contents.some((content) => content.kind === 'plugin_content' && content.contentRef && content.preview == null)).toBeTruthy()
  const executionEvidence = process.env.AGENTX_PLUGIN_EXECUTION_OUTPUT
  if (executionEvidence) await writeFile(executionEvidence, JSON.stringify({ executionId, workflowId: workflow.id }), 'utf8')

  const parentWorkflowResponse = await page.request.post('/api/v1/workflows', { headers: { Authorization: `Bearer ${token}` }, data: { name: `Plugin sub-workflow ${Date.now()}`, visibility: 'company' } })
  expect(parentWorkflowResponse.status()).toBe(201)
  const parentWorkflow = await parentWorkflowResponse.json() as { id: string }
  const parentDraft = await api<{ revision: number; definition: { nodes: unknown[]; connections: unknown[]; [key: string]: unknown }; editorDocument: { nodeLayouts?: unknown[]; [key: string]: unknown } }>(page, token, `/workflows/${parentWorkflow.id}/drafts/${await fixtureDraftId(page, parentWorkflow.id)}`)
  const childNode = { id: 'plugin-child', key: 'plugin_child', type: 'sub_workflow', typeVersion: 1, name: 'Plugin child', disabled: false, protected: false, parameters: { workflowVersionId: workflowVersion.id, inputs: { kind: 'object', fields: {} } }, contextWrites: [], resourceReferences: [], settings: {} }
  const parentDefinition = { ...parentDraft.definition, nodes: [childNode, ...parentDraft.definition.nodes], connections: [{ id: 'parent-start', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'plugin-child', targetHandle: 'main', order: 0 }, { id: 'parent-end', sourceNodeId: 'plugin-child', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 }] }
  const parentEditor = { ...parentDraft.editorDocument, nodeLayouts: [{ nodeId: 'plugin-child', x: 320, y: 200 }, ...(parentDraft.editorDocument.nodeLayouts ?? [])], edges: [{ edgeId: 'parent-start' }, { edgeId: 'parent-end' }] }
  const parentSave = await page.request.put(`/api/v1/workflows/${parentWorkflow.id}/drafts/${await fixtureDraftId(page, parentWorkflow.id)}`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': crypto.randomUUID() }, data: { expectedRevision: parentDraft.revision, definition: parentDefinition, editorDocument: parentEditor } })
  expect(parentSave.ok(), await parentSave.text()).toBeTruthy()
  const parentRevision = ((await parentSave.json()) as { revision: number }).revision
  const parentDebug = await page.request.post(`/api/v1/workflows/${parentWorkflow.id}/drafts/${await fixtureDraftId(page, parentWorkflow.id)}/debug-executions`, { headers: { Authorization: `Bearer ${token}` }, data: { expectedRevision: parentRevision, idempotencyKey: crypto.randomUUID(), input: {}, context: {}, mode: 'full', targetNodeId: null, inputSource: {}, overlayIds: [], sideEffectDecisions: {} } })
  expect(parentDebug.status(), await parentDebug.text()).toBe(202)
  const parentExecutionId = ((await parentDebug.json()) as { executionId: string }).executionId
  await expect.poll(async () => (await api<{ status: string }>(page, token, `/executions/${parentExecutionId}`)).status, { timeout: 120_000 }).toBe('succeeded')
  const parentNodes = await api<{ items: Array<{ nodeType: string; status: string }> }>(page, token, `/executions/${parentExecutionId}/nodes`)
  expect(parentNodes.items).toContainEqual(expect.objectContaining({ nodeType: 'sub_workflow', status: 'succeeded' }))
  const evaluationSuffix = Date.now().toString()
  const datasetResponse = await page.request.post('/api/v1/datasets', { headers: { Authorization: `Bearer ${token}` }, data: { name: `Plugin dataset ${evaluationSuffix}`, description: 'Plugin closure', visibility: 'company' } })
  expect(datasetResponse.status()).toBe(201)
  const dataset = await datasetResponse.json() as { id: string }
  const caseResponse = await page.request.post(`/api/v1/datasets/${dataset.id}/cases`, { headers: { Authorization: `Bearer ${token}` }, data: { expectedRevision: 0, caseKey: 'plugin-case', name: 'Plugin case', input: {}, expectedOutput: { mapped: 1 }, context: null, tags: ['plugin'], evaluatorOverride: null } })
  expect(caseResponse.status()).toBe(201)
  const datasetVersionResponse = await page.request.post(`/api/v1/datasets/${dataset.id}/versions`, { headers: { Authorization: `Bearer ${token}` } })
  expect(datasetVersionResponse.status()).toBe(201)
  const datasetVersion = await datasetVersionResponse.json() as { id: string }
  const profileResponse = await page.request.post('/api/v1/evaluation-profiles', { headers: { Authorization: `Bearer ${token}` }, data: { name: `Plugin exact ${evaluationSuffix}`, description: 'Plugin closure', visibility: 'company', aggregation: 'all', passThreshold: '1', rules: [{ key: 'exact', name: 'Exact', evaluatorType: 'exact', configuration: {}, weight: '1', required: true }] } })
  expect(profileResponse.status()).toBe(201)
  const profile = await profileResponse.json() as { versionId: string }
  const evaluationResponse = await page.request.post('/api/v1/evaluations', { headers: { Authorization: `Bearer ${token}` }, data: { name: `Plugin evaluation ${evaluationSuffix}`, workflowVersionId: workflowVersion.id, datasetVersionId: datasetVersion.id, evaluationProfileVersionId: profile.versionId, visibility: 'company', parameters: {} } })
  expect(evaluationResponse.status()).toBe(201)
  const evaluation = await evaluationResponse.json() as { id: string }
  const evaluationStart = await page.request.post(`/api/v1/evaluations/${evaluation.id}/start`, { headers: { Authorization: `Bearer ${token}` } })
  expect(evaluationStart.status()).toBe(202)
  await expect.poll(async () => (await page.request.get(`/api/v1/evaluations/${evaluation.id}/report`, { headers: { Authorization: `Bearer ${token}` } })).status(), { timeout: 180_000, intervals: [500, 1_000, 2_000] }).toBe(200)

  const installed = (await api<{ items: Array<{ id: string; packageId: string; version: number; versions: Array<{ id: string; packageVersion: string; bundleDigest: string }> }> }>(page, token, '/canvas-plugins?pageSize=100')).items.find((plugin) => plugin.packageId === 'acme/json-mapper')!
  const version = installed.versions.find((candidate) => candidate.packageVersion === '1.0.0')!
  const departments = await api<Array<{ id: string; isRoot: boolean }>>(page, token, '/departments')
  const roleResponse = await page.request.post('/api/v1/roles', { headers: { Authorization: `Bearer ${token}` }, data: { code: `plugin_designer_${Date.now()}`, name: 'Plugin Designer', description: 'Uses installed nodes without package management', dataScope: 'company', permissions: ['workflow:view', 'workflow:create', 'workflow:edit', 'execution:run', 'execution:view', 'canvas_plugin:view'] } })
  expect(roleResponse.status()).toBe(201)
  const role = await roleResponse.json() as { id: string }
  const username = `plugin_designer_${Date.now()}`
  const userResponse = await page.request.post('/api/v1/users', { headers: { Authorization: `Bearer ${token}` }, data: { username, displayName: 'Plugin Designer', departmentId: departments.find((item) => item.isRoot)!.id, roleId: role.id } })
  expect(userResponse.status()).toBe(201)
  const designerLogin = await page.request.post('/api/v1/auth/login', { data: { username, password: '123456' } })
  expect(designerLogin.ok(), await designerLogin.text()).toBeTruthy()
  const designerLoginBody = await designerLogin.json() as { accessToken?: string; passwordChangeRequired: boolean; changePasswordToken?: string }
  let designerToken = designerLoginBody.accessToken
  if (designerLoginBody.passwordChangeRequired) {
    const changed = await page.request.post('/api/v1/auth/change-password', { data: { token: designerLoginBody.changePasswordToken, password: 'plugin-designer-password' } })
    expect(changed.ok(), await changed.text()).toBeTruthy()
    designerToken = ((await changed.json()) as { accessToken: string }).accessToken
  }
  expect(designerToken).toBeTruthy()
  expect((await page.request.get('/api/v1/canvas-plugins?pageSize=100', { headers: { Authorization: `Bearer ${designerToken}` } })).status()).toBe(200)
  expect((await page.request.get('/api/v1/node-definitions?pageSize=100', { headers: { Authorization: `Bearer ${designerToken}` } })).status()).toBe(200)
  expect((await page.request.get(`/api/v1/canvas-plugins/${installed.id}/versions/${version.id}/download`, { headers: { Authorization: `Bearer ${designerToken}` } })).status()).toBe(403)
  expect((await page.request.post('/api/v1/canvas-plugin-imports', { headers: { Authorization: `Bearer ${designerToken}` }, multipart: { file: { name: 'denied.agentx-plugin', mimeType: 'application/zip', buffer: packageBytes } } })).status()).toBe(403)
  const designerWorkflowResponse = await page.request.post('/api/v1/workflows', { headers: { Authorization: `Bearer ${designerToken}` }, data: { name: `Designer plugin ${Date.now()}`, visibility: 'company' } })
  expect(designerWorkflowResponse.status()).toBe(201)
  const designerWorkflow = await designerWorkflowResponse.json() as { id: string }
  const designerDraft = await api<{ revision: number; definition: { nodes: unknown[]; connections: unknown[]; [key: string]: unknown }; editorDocument: { nodeLayouts?: unknown[]; [key: string]: unknown } }>(page, designerToken!, `/workflows/${designerWorkflow.id}/drafts/${await fixtureDraftId(page, designerWorkflow.id)}`)
  const designerNode = { id: 'designer-plugin', key: 'designer_plugin', type: 'acme.json_mapper', typeVersion: 1, name: 'Designer Plugin', disabled: false, protected: false, parameters: { label: 'designer', includeMetadata: false, largeTrace: false }, contextWrites: [], resourceReferences: [], settings: {} }
  const designerDefinition = { ...designerDraft.definition, nodes: [designerNode, ...designerDraft.definition.nodes], connections: [{ id: 'designer-start', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'designer-plugin', targetHandle: 'main', order: 0 }, { id: 'designer-end', sourceNodeId: 'designer-plugin', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 }] }
  const designerEditor = { ...designerDraft.editorDocument, nodeLayouts: [{ nodeId: 'designer-plugin', x: 320, y: 200 }, ...(designerDraft.editorDocument.nodeLayouts ?? [])], edges: [{ edgeId: 'designer-start' }, { edgeId: 'designer-end' }] }
  const designerSave = await page.request.put(`/api/v1/workflows/${designerWorkflow.id}/drafts/${await fixtureDraftId(page, designerWorkflow.id)}`, { headers: { Authorization: `Bearer ${designerToken}`, 'Idempotency-Key': crypto.randomUUID() }, data: { expectedRevision: designerDraft.revision, definition: designerDefinition, editorDocument: designerEditor } })
  expect(designerSave.ok(), await designerSave.text()).toBeTruthy()
  const designerRevision = ((await designerSave.json()) as { revision: number }).revision
  const designerDebug = await page.request.post(`/api/v1/workflows/${designerWorkflow.id}/drafts/${await fixtureDraftId(page, designerWorkflow.id)}/debug-executions`, { headers: { Authorization: `Bearer ${designerToken}` }, data: { expectedRevision: designerRevision, idempotencyKey: crypto.randomUUID(), input: {}, context: {}, mode: 'full', targetNodeId: null, inputSource: {}, overlayIds: [], sideEffectDecisions: {} } })
  expect(designerDebug.status(), await designerDebug.text()).toBe(202)
  const designerRun = (await designerDebug.json()) as { debugRunId: string; executionId: string }
  await expect.poll(async () => {
    const runs = await api<{ items: Array<{ id: string; status: string }> }>(page, token, `/workflows/${designerWorkflow.id}/debug-runs`)
    return runs.items.find((item) => item.id === designerRun.debugRunId)?.status ?? 'pending'
  }, { timeout: 120_000, intervals: [250, 500, 1_000, 2_000] }).toMatch(/succeeded|completed/)
  const restoredAdmin = await page.request.post('/api/v1/auth/login', { data: { username: 'admin', password } })
  expect(restoredAdmin.ok(), await restoredAdmin.text()).toBeTruthy()
  const references = await api<{ drafts: number; total: number }>(page, token, `/canvas-plugins/${installed.id}/references`)
  expect(references.drafts).toBeGreaterThan(0)
  expect(references.total).toBeGreaterThan(0)
  const ongoingExecution = await startProduction(`plugin-running-during-disable-${suffix}`)
  expect(ongoingExecution.status).not.toBe('completed')
  const disabled = await page.request.patch(`/api/v1/canvas-plugins/${installed.id}/versions/${version.id}`, {
    headers: { Authorization: `Bearer ${token}` }, data: { enabled: false, expectedRevision: installed.version },
  })
  expect(disabled.ok(), await disabled.text()).toBeTruthy()
  const disabledPlugin = await disabled.json() as { version: number }
  expect((await waitInvocation(page, apiKey.secret, ongoingExecution.id)).status).toBe('completed')
  expect((await invokeProduction(`plugin-after-disable-${suffix}`)).outputs?.mapped).toBe(1)
  const blockedRepublish = await page.request.post(`/api/v1/applications/${application.id}/deployments`, { headers: { Authorization: `Bearer ${token}` }, data: { workflowVersionId: workflowVersion.id, environmentId: environment.id, sessionVersionPolicy: 'pinned' } })
  expect(blockedRepublish.status()).toBe(409)
  expect((await blockedRepublish.json()).code).toBe('PLUGIN_VERSION_DISABLED')
  const blockedDebug = await page.request.post(`/api/v1/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}/debug-executions`, { headers: { Authorization: `Bearer ${token}` }, data: { expectedRevision: savedRevision, idempotencyKey: crypto.randomUUID(), input: {}, context: {}, mode: 'full', targetNodeId: null, inputSource: {}, overlayIds: [], sideEffectDecisions: {} } })
  expect(blockedDebug.status()).toBe(409)
  expect((await blockedDebug.json()).code).toBe('PLUGIN_VERSION_DISABLED')
  const catalog = await api<{ items: Array<{ nodeType: string; version: number }> }>(page, token, '/node-definitions?pageSize=100')
  expect(catalog.items.some((item) => item.nodeType === 'acme.json_mapper' && item.version === 1)).toBeFalsy()
  const frozenManifest = await page.request.get(`/api/v1/node-definitions/acme.json_mapper/versions/1?bundleDigest=${encodeURIComponent(version.bundleDigest)}`, { headers: { Authorization: `Bearer ${token}` } })
  expect(frozenManifest.ok(), await frozenManifest.text()).toBeTruthy()
  await page.goto(`/executions/${executionId}`)
  const historicalTrace = page.getByTestId('trace-workspace')
  await historicalTrace.getByRole('tab', { name: '高级瀑布' }).click()
  const historicalSpan = historicalTrace.getByRole('row').filter({ hasText: 'Map items' }).first()
  await expect(historicalSpan).toBeVisible({ timeout: 30_000 })
  await historicalSpan.click()
  await historicalTrace.getByRole('tab', { name: '插件业务内容' }).click()
  await expect(historicalTrace.getByText('Mapped items: 1')).toBeVisible()
  const traceShot = testInfo.outputPath('plugin-trace-light-zh.png')
  await page.screenshot({ path: traceShot, fullPage: true })
  await testInfo.attach('plugin-trace-light-zh.png', { path: traceShot, contentType: 'image/png' })
  const checkpoints = await api<{ items: Array<{ id: string }> }>(page, token, `/executions/${executionId}/checkpoints`)
  expect(checkpoints.items.length).toBeGreaterThan(0)
  const disabledFork = await page.request.post(`/api/v1/executions/${executionId}/fork`, { headers: { Authorization: `Bearer ${token}` }, data: { checkpointId: checkpoints.items.at(-1)!.id, mode: 'whole', nodeId: null, sideEffectDecisions: {}, idempotencyKey: crypto.randomUUID() } })
  expect(disabledFork.status()).toBe(409)
  expect((await disabledFork.json()).code).toBe('PLUGIN_VERSION_DISABLED')
  const blockedDelete = await page.request.delete(`/api/v1/canvas-plugins/${installed.id}/versions/${version.id}`, { headers: { Authorization: `Bearer ${token}` } })
  expect(blockedDelete.status()).toBe(409)
  expect((await blockedDelete.json()).code).toBe('PLUGIN_VERSION_IN_USE')
  const reenabled = await page.request.patch(`/api/v1/canvas-plugins/${installed.id}/versions/${version.id}`, {
    headers: { Authorization: `Bearer ${token}` }, data: { enabled: true, expectedRevision: disabledPlugin.version },
  })
  expect(reenabled.ok(), await reenabled.text()).toBeTruthy()
  const download = await page.request.get(`/api/v1/canvas-plugins/${installed.id}/versions/${version.id}/download`, { headers: { Authorization: `Bearer ${token}` } })
  expect(download.ok(), await download.text()).toBeTruthy()
  expect(await download.body()).toEqual(packageBytes)

  const duplicateImport = await page.request.post('/api/v1/canvas-plugin-imports', {
    headers: { Authorization: `Bearer ${token}` },
    multipart: { file: { name: 'same.agentx-plugin', mimeType: 'application/zip', buffer: packageBytes } },
  })
  expect(duplicateImport.ok(), await duplicateImport.text()).toBeTruthy()
  const duplicate = await duplicateImport.json() as { id: string; bundleDigest: string }
  const duplicateInstall = await page.request.post(`/api/v1/canvas-plugin-imports/${duplicate.id}/install`, {
    headers: { Authorization: `Bearer ${token}` }, data: { bundleDigest: duplicate.bundleDigest, enable: true, setDefault: true },
  })
  expect(duplicateInstall.status()).toBe(200)

  const changedBytes = Buffer.concat([packageBytes, Buffer.from('different-approved-build')])
  const conflictImport = await page.request.post('/api/v1/canvas-plugin-imports', {
    headers: { Authorization: `Bearer ${token}` },
    multipart: { file: { name: 'changed.agentx-plugin', mimeType: 'application/zip', buffer: changedBytes } },
  })
  expect(conflictImport.ok(), await conflictImport.text()).toBeTruthy()
  const changed = await conflictImport.json() as { id: string; bundleDigest: string }
  const conflictInstall = await page.request.post(`/api/v1/canvas-plugin-imports/${changed.id}/install`, {
    headers: { Authorization: `Bearer ${token}` }, data: { bundleDigest: changed.bundleDigest, enable: true, setDefault: true },
  })
  expect(conflictInstall.status()).toBe(409)
  expect((await conflictInstall.json()).code).toBe('PLUGIN_VERSION_CONFLICT')
  const cancelled = await page.request.delete(`/api/v1/canvas-plugin-imports/${changed.id}`, { headers: { Authorization: `Bearer ${token}` } })
  expect(cancelled.status()).toBe(204)
  const retried = await page.request.post('/api/v1/canvas-plugin-imports', {
    headers: { Authorization: `Bearer ${token}` },
    multipart: { file: { name: 'changed.agentx-plugin', mimeType: 'application/zip', buffer: changedBytes } },
  })
  expect(retried.status()).toBe(201)

  const raceBytes = await readFile(resolve(import.meta.dirname, '../../../src/plugins/templates/canvas-plugin/dist/acme-race.agentx-plugin'))
  let racePlugin = (await api<{ items: Array<{ id: string; packageId: string; versions: Array<{ id: string }> }> }>(page, token, '/canvas-plugins?pageSize=100')).items.find((item) => item.packageId === 'acme/race')
  if (!racePlugin) {
    const raceImportResponse = await page.request.post('/api/v1/canvas-plugin-imports', { headers: { Authorization: `Bearer ${token}` }, multipart: { file: { name: 'race.agentx-plugin', mimeType: 'application/zip', buffer: raceBytes } } })
    expect(raceImportResponse.status()).toBe(201)
    const raceImport = await raceImportResponse.json() as { id: string; bundleDigest: string }
    const raceInstallResponse = await page.request.post(`/api/v1/canvas-plugin-imports/${raceImport.id}/install`, { headers: { Authorization: `Bearer ${token}` }, data: { bundleDigest: raceImport.bundleDigest, enable: true, setDefault: true } })
    expect(raceInstallResponse.status()).toBe(201)
    racePlugin = await raceInstallResponse.json() as { id: string; packageId: string; versions: Array<{ id: string }> }
  }
  const raceWorkflowResponse = await page.request.post('/api/v1/workflows', { headers: { Authorization: `Bearer ${token}` }, data: { name: `Plugin delete race ${Date.now()}`, visibility: 'company' } })
  const raceWorkflow = await raceWorkflowResponse.json() as { id: string }
  const raceDraft = await api<{ revision: number; definition: { nodes: unknown[]; connections: unknown[]; [key: string]: unknown }; editorDocument: { nodeLayouts: unknown[]; edges: unknown[]; [key: string]: unknown } }>(page, token, `/workflows/${raceWorkflow.id}/drafts/${await fixtureDraftId(page, raceWorkflow.id)}`)
  const raceNode = { id: 'race', key: 'race', type: 'acme.race', typeVersion: 1, name: 'Race Plugin', disabled: false, protected: false, parameters: { label: 'race', includeMetadata: false }, contextWrites: [], resourceReferences: [], settings: {} }
  const raceDefinition = { ...raceDraft.definition, nodes: [raceNode, ...raceDraft.definition.nodes], connections: [{ id: 'race-start', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'race', targetHandle: 'main', order: 0 }, { id: 'race-exit', sourceNodeId: 'race', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 }] }
  const raceEditor = { ...raceDraft.editorDocument, nodeLayouts: [{ nodeId: 'race', x: 340, y: 220 }, ...raceDraft.editorDocument.nodeLayouts], edges: [{ edgeId: 'race-start' }, { edgeId: 'race-exit' }] }
  const [raceSave, raceDelete] = await Promise.all([
    page.request.put(`/api/v1/workflows/${raceWorkflow.id}/drafts/${await fixtureDraftId(page, raceWorkflow.id)}`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': crypto.randomUUID() }, data: { expectedRevision: raceDraft.revision, definition: raceDefinition, editorDocument: raceEditor } }),
    page.request.delete(`/api/v1/canvas-plugins/${racePlugin.id}/versions/${racePlugin.versions[0].id}`, { headers: { Authorization: `Bearer ${token}` } }),
  ])
  expect([raceSave.ok(), raceDelete.ok()].filter(Boolean)).toHaveLength(1)
  expect([raceSave.status(), raceDelete.status()].some((status) => status === 409 || status === 422)).toBeTruthy()
  const raceReimportResponse = await page.request.post('/api/v1/canvas-plugin-imports', { headers: { Authorization: `Bearer ${token}` }, multipart: { file: { name: 'race-reinstall.agentx-plugin', mimeType: 'application/zip', buffer: raceBytes } } })
  expect([200, 201]).toContain(raceReimportResponse.status())
  const raceReimport = await raceReimportResponse.json() as { id: string; bundleDigest: string }
  const raceReinstall = await page.request.post(`/api/v1/canvas-plugin-imports/${raceReimport.id}/install`, { headers: { Authorization: `Bearer ${token}` }, data: { bundleDigest: raceReimport.bundleDigest, enable: true, setDefault: true } })
  expect([200, 201], await raceReinstall.text()).toContain(raceReinstall.status())

  const v2Path = resolve(import.meta.dirname, '../../../src/plugins/templates/canvas-plugin/dist/acme-json-mapper-v2.agentx-plugin')
  const v2Import = await page.request.post('/api/v1/canvas-plugin-imports', {
    headers: { Authorization: `Bearer ${token}` },
    multipart: { file: { name: 'json-mapper-v2.agentx-plugin', mimeType: 'application/zip', buffer: await readFile(v2Path) } },
  })
  expect(v2Import.ok(), await v2Import.text()).toBeTruthy()
  const v2Preview = await v2Import.json() as { id: string; bundleDigest: string }
  const v2Installs = await Promise.all([0, 1].map(() => page.request.post(`/api/v1/canvas-plugin-imports/${v2Preview.id}/install`, {
    headers: { Authorization: `Bearer ${token}` }, data: { bundleDigest: v2Preview.bundleDigest, enable: true, setDefault: true },
  })))
  const installStatuses = v2Installs.map((response) => response.status()).sort()
  expect([[200, 200], [200, 201]]).toContainEqual(installStatuses)
  const v2Install = v2Installs.find((response) => response.status() === 201) ?? v2Installs[0]
  let withV2 = await v2Install.json() as { id: string; version: number; defaultVersionId: string; versions: Array<{ id: string; packageVersion: string; status: string }> }
  const existingV2 = withV2.versions.find((item) => item.packageVersion === '2.0.0')!
  if (existingV2.status !== 'enabled') {
    const enabledV2 = await page.request.patch(`/api/v1/canvas-plugins/${withV2.id}/versions/${existingV2.id}`, {
      headers: { Authorization: `Bearer ${token}` }, data: { enabled: true, expectedRevision: withV2.version },
    })
    expect(enabledV2.ok(), await enabledV2.text()).toBeTruthy()
    withV2 = await enabledV2.json() as typeof withV2
  }
  const enabledV2 = withV2.versions.find((item) => item.packageVersion === '2.0.0')!
  if (withV2.defaultVersionId !== enabledV2.id) {
    const defaultV2 = await page.request.patch(`/api/v1/canvas-plugins/${withV2.id}`, {
      headers: { Authorization: `Bearer ${token}` }, data: { defaultVersionId: enabledV2.id, expectedRevision: withV2.version },
    })
    expect(defaultV2.ok(), await defaultV2.text()).toBeTruthy()
    withV2 = await defaultV2.json() as typeof withV2
  }
  expect(withV2.versions.map((item) => item.packageVersion)).toEqual(expect.arrayContaining(['1.0.0', '2.0.0']))
  expect(withV2.versions.find((item) => item.id === withV2.defaultVersionId)?.packageVersion).toBe('2.0.0')
  const unchangedDraft = await api<{ definition: { nodes: Array<{ type: string; typeVersion: number }> } }>(page, token, `/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`)
  expect(unchangedDraft.definition.nodes.find((node) => node.type === 'acme.json_mapper')?.typeVersion).toBe(1)

  await page.goto(`/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}/editor`)
  const versionedNode = page.locator('.react-flow__node-manifest').filter({ hasText: 'JSON Mapper' })
  await versionedNode.click()
  const versionField = page.getByTestId('node-details-view').locator('[data-field-path="typeVersion"]')
  await expect(versionField).toBeVisible()
  await versionField.getByRole('combobox').click()
  await page.getByRole('option', { name: /2\.0\.0/ }).click()
  const impact = page.getByRole('dialog', { name: '插件版本影响预览' })
  await expect(impact.getByText('同包节点')).toBeVisible()
  await impact.getByRole('button', { name: '确认切换' }).click()
  const versionSave = page.locator('header').getByRole('button', { name: '保存', exact: true })
  await expect(versionSave).toBeEnabled()
  const versionSaved = page.waitForResponse((value) => value.url().endsWith(`/api/v1/workflows/${workflow.id}/drafts/${fixtureDraftIdKnown(workflow.id)}`) && value.request().method() === 'PUT')
  await versionSave.click()
  expect((await versionSaved).ok()).toBeTruthy()
  const switchedDraft = await api<{ definition: { nodes: Array<{ type: string; typeVersion: number; parameters: { label?: string } }> } }>(page, token, `/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`)
  expect(switchedDraft.definition.nodes.find((node) => node.type === 'acme.json_mapper')).toMatchObject({ typeVersion: 2, parameters: { label: 'customer' } })
  const v2ExecutionResponse = page.waitForResponse((value) => value.url().includes('/debug-executions') && value.request().method() === 'POST')
  await page.locator('header').getByRole('button', { name: '运行', exact: true }).click()
  const v2RunDialog = page.getByRole('dialog', { name: /运行工作流|调试输入/ })
  if (await v2RunDialog.isVisible().catch(() => false)) await v2RunDialog.getByRole('button', { name: /开始运行|运行/ }).click()
  const v2Accepted = await v2ExecutionResponse
  expect(v2Accepted.status()).toBe(202)
  const v2ExecutionId = ((await v2Accepted.json()) as { executionId: string }).executionId
  await expect.poll(async () => (await api<{ status: string }>(page, token, `/executions/${v2ExecutionId}`)).status, { timeout: 120_000 }).toBe('succeeded')
  const v2Nodes = await api<{ items: Array<{ nodeType: string; output?: { main?: Array<{ json: Record<string, unknown> }> } }> }>(page, token, `/executions/${v2ExecutionId}/nodes`)
  expect(v2Nodes.items.find((node) => node.nodeType === 'acme.json_mapper')?.output?.main?.[0].json.pluginVersion).toBe('2.0.0')
  const secondWorkflowResponse = await page.request.post('/api/v1/workflows', { headers: { Authorization: `Bearer ${token}` }, data: { name: `Plugin v2 workflow ${Date.now()}`, visibility: 'company' } })
  expect(secondWorkflowResponse.status()).toBe(201)
  const secondWorkflow = await secondWorkflowResponse.json() as { id: string }
  const secondDraft = await api<{ revision: number; definition: { nodes: unknown[]; connections: unknown[]; [key: string]: unknown }; editorDocument: { nodeLayouts?: unknown[]; [key: string]: unknown } }>(page, token, `/workflows/${secondWorkflow.id}/drafts/${await fixtureDraftId(page, secondWorkflow.id)}`)
  const secondNode = { id: 'plugin-v2', key: 'plugin_v2', type: 'acme.json_mapper', typeVersion: 2, name: 'Plugin v2', disabled: false, protected: false, parameters: { label: 'second', includeMetadata: false, largeTrace: false }, contextWrites: [], resourceReferences: [], settings: {} }
  const secondDefinition = { ...secondDraft.definition, nodes: [secondNode, ...secondDraft.definition.nodes], connections: [{ id: 'second-start', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: 'plugin-v2', targetHandle: 'main', order: 0 }, { id: 'second-end', sourceNodeId: 'plugin-v2', sourceHandle: 'main', targetNodeId: 'exit', targetHandle: 'main', order: 0 }] }
  const secondEditor = { ...secondDraft.editorDocument, nodeLayouts: [{ nodeId: 'plugin-v2', x: 320, y: 200 }, ...(secondDraft.editorDocument.nodeLayouts ?? [])], edges: [{ edgeId: 'second-start' }, { edgeId: 'second-end' }] }
  const secondSave = await page.request.put(`/api/v1/workflows/${secondWorkflow.id}/drafts/${await fixtureDraftId(page, secondWorkflow.id)}`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': crypto.randomUUID() }, data: { expectedRevision: secondDraft.revision, definition: secondDefinition, editorDocument: secondEditor } })
  expect(secondSave.ok(), await secondSave.text()).toBeTruthy()
  const secondRevision = ((await secondSave.json()) as { revision: number }).revision
  const secondDebug = await page.request.post(`/api/v1/workflows/${secondWorkflow.id}/drafts/${await fixtureDraftId(page, secondWorkflow.id)}/debug-executions`, { headers: { Authorization: `Bearer ${token}` }, data: { expectedRevision: secondRevision, idempotencyKey: crypto.randomUUID(), input: {}, context: {}, mode: 'full', targetNodeId: null, inputSource: {}, overlayIds: [], sideEffectDecisions: {} } })
  expect(secondDebug.status(), await secondDebug.text()).toBe(202)
  const secondExecutionId = ((await secondDebug.json()) as { executionId: string }).executionId
  await expect.poll(async () => (await api<{ status: string }>(page, token, `/executions/${secondExecutionId}`)).status, { timeout: 120_000 }).toBe('succeeded')
  const secondNodes = await api<{ items: Array<{ nodeType: string; output?: { main?: Array<{ json: Record<string, unknown> }> } }> }>(page, token, `/executions/${secondExecutionId}/nodes`)
  expect(secondNodes.items.find((node) => node.nodeType === 'acme.json_mapper')?.output?.main?.[0].json.pluginVersion).toBe('2.0.0')
  expect((await invokeProduction(`plugin-v1-after-v2-${suffix}`)).outputs?.mapped).toBe(1)
  await page.evaluate(() => { localStorage.setItem('agentx.locale', 'en-US'); localStorage.setItem('agentx.theme', 'dark') })
  await page.goto('/canvas-plugins')
  await expect(page.getByRole('heading', { name: 'Canvas Plugins' })).toBeVisible()
  const darkShot = testInfo.outputPath('canvas-plugins-dark-en.png')
  await page.screenshot({ path: darkShot, fullPage: true })
  await testInfo.attach('canvas-plugins-dark-en.png', { path: darkShot, contentType: 'image/png' })
  for (const [locale, theme, heading] of [['zh-CN', 'dark', '画布插件'], ['en-US', 'light', 'Canvas Plugins']] as const) {
    await page.evaluate(([nextLocale, nextTheme]) => { localStorage.setItem('agentx.locale', nextLocale); localStorage.setItem('agentx.theme', nextTheme) }, [locale, theme])
    await page.reload()
    await expect(page.getByRole('heading', { name: heading })).toBeVisible()
    const shot = testInfo.outputPath(`canvas-plugins-${theme}-${locale}.png`)
    await page.screenshot({ path: shot, fullPage: true })
    await testInfo.attach(`canvas-plugins-${theme}-${locale}.png`, { path: shot, contentType: 'image/png' })
  }
  const unnamedButtons = await page.locator('button').evaluateAll((buttons) => buttons.filter((button) => !(button.getAttribute('aria-label') || button.textContent?.trim())).length)
  expect(unnamedButtons).toBe(0)
})
