import { fixtureVersionId } from '../draft-fixtures'
import { expect, type Page } from '@playwright/test'
import { createHash } from 'node:crypto'
import type { components } from '../../../src/web/src/shared/api/generated'
import { canonicalDocument } from '../../../src/web/src/features/workflow-designer/model/document-hash'
import { allEntries, completed, count, create, login, read, saved, send, setModel, test, type Created } from '../studio-builder-support'

type Document = components['schemas']['StudioEditorSnapshotV1']['document']
type Draft = Document & { revision: number }
const edge = (sourceNodeId: string, targetNodeId: string, sourceHandle = 'main', targetHandle = 'main') => ({ id: `${sourceNodeId}-${sourceHandle}-${targetNodeId}`, sourceNodeId, targetNodeId, sourceHandle, targetHandle, order: 0 })
const reference = (namespace: 'outputs' | 'item', path: string[], sourceNodeId?: string) => ({ kind: 'reference' as const, selector: { namespace, path, ...(sourceNodeId ? { sourceNodeId } : {}) } })

function expectErrorExit(draft: Draft, sourceId: string) {
  const connection = draft.definition.connections.find(edge => edge.sourceNodeId === sourceId && edge.sourceHandle === 'error')
  expect(connection?.targetHandle).toBe('error')
  const exit = draft.definition.nodes.find(node => node.id === connection?.targetNodeId)
  expect(exit?.type).toBe('exit')
  expect(exit?.protected).not.toBe(true)
  expect(draft.definition.connections.some(edge => edge.sourceNodeId === sourceId && edge.sourceHandle === 'main' && edge.targetNodeId === exit?.id)).toBe(false)
  const parameters = exit?.parameters as { errorOutputs?: Record<string, unknown> } | undefined
  expect(Object.keys(parameters?.errorOutputs ?? {}).length).toBeGreaterThan(0)
}

async function namedModel(page: Page, token: string, created: Created, name: string) {
  const options = await read<{ items: Array<{ id: string; name: string; resourceVersionId: string }> }>(page, token, `/workflows/${created.id}/resource-options?draftId=${created.documentId}&resourceType=model&operation=use&pageSize=100`)
  const model = options.items.find(item => item.name === name)
  expect(model).toBeTruthy()
  return model!
}

async function document(page: Page, token: string, created: Created, resourceId?: string): Promise<Document> {
  const original = await read<Draft>(page, token, `/workflows/${created.id}/drafts/${created.documentId}`)
  const result: Document = { definition: structuredClone(original.definition), editorDocument: structuredClone(original.editorDocument) }
  const success = result.definition.nodes.find(node => node.type === 'exit')!
  success.parameters = { outputs: { answer: reference('outputs', ['text'], 'retrieve'), documents: reference('outputs', ['documents'], 'retrieve'), files: reference('outputs', ['files'], 'retrieve') }, errorOutputs: {} }
  result.definition.end.outputs = Object.fromEntries(['answer', 'documents', 'files'].map(name => [name, { schema: name === 'answer' ? { type: 'string' } : { type: 'array', items: { type: 'object' } }, required: true, sensitive: false }]))
  result.definition.end.error.outputs = Object.fromEntries(['code', 'message', 'source_node_id'].map(name => [name, { schema: { type: 'string' }, required: true, sensitive: false }]))
  result.definition.nodes.push({ ...success, protected: false, id: 'retrieve', key: 'retrieve', name: 'Retrieve knowledge', type: 'knowledge_retrieval', typeVersion: 1, parameters: { query: { kind: 'literal', value: 'launch code' }, topK: 2 }, settings: success.settings, contextWrites: [], resourceReferences: resourceId ? [{ resourceType: 'rag', resourceId, operation: 'read' }] : [] })
  const parameters = success.parameters as Record<string, unknown>
  parameters.errorOutputs = Object.fromEntries(['code', 'message', 'source_node_id'].map(name => [name, reference('item', [name === 'source_node_id' ? 'sourceNodeId' : name])]))
  result.definition.connections = [edge('__start__', 'retrieve'), edge('retrieve', success.id), edge('retrieve', success.id, 'error', 'error')]
  return result
}

async function apply(page: Page, value: Document) {
  await send(page, `P8FIX: inspector=${JSON.stringify(value)}`)
  await completed(page)
}

async function run(page: Page, token: string, created: Created, status: 'succeeded' | 'failed') {
  const accepted = page.waitForResponse(value => value.url().endsWith(`/drafts/${created.documentId}/debug-executions`) && value.request().method() === 'POST')
  await page.getByRole('button', { name: '运行', exact: true }).first().click()
  const response = await accepted
  expect(response.status(), await response.text()).toBe(202)
  const { executionId } = await response.json() as { executionId: string }
  await expect.poll(async () => (await read<{ status: string }>(page, token, `/executions/${executionId}`)).status, { timeout: 90_000 }).toBe(status)
  await page.getByRole('tab', { name: 'Trace', exact: true }).click()
  await expect(page.getByTestId('trace-workspace')).toBeVisible()
  await expect(page.getByTestId('trace-final-output')).toContainText(status === 'failed' ? '失败' : '成功')
  await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0)
  await page.getByRole('tab', { name: '事件', exact: true }).click()
  await expect(page.getByTestId('execution-events')).toContainText(`execution.${status}`)
  return read<Record<string, unknown>>(page, token, `/executions/${executionId}`)
}

for (const provider of ['lightrag', 'ragflow']) {
  test(`P8 knowledge ${provider}: save, grants, text/files, empty retrieval and error Exit`, async ({ page }, info) => {
    const token = await login(page)
    await setModel(page, process.env.AGENTX_E2E_STUDIO_FIXTURE_ALIAS!)
    const created = await create(page, `P8 ${provider} ${Date.now()}`)
    const resourceId = process.env[`AGENTX_E2E_STUDIO_KNOWLEDGE_${provider.toUpperCase()}`]!
    let value = await document(page, token, created, resourceId)
    await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
    await apply(page, value)
    const applied = await saved(page, token, created) as unknown as Draft
    value = { definition: applied.definition, editorDocument: applied.editorDocument }
    expect(await count(page, token, created.id)).toBe(0)
    // Setup grants use the same authorization endpoint as the editor. The AI
    // has deliberately left the ungranted resource selected for user action.
    const grant = await page.request.post(`/api/v1/workflows/${created.id}/resource-authorizations`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() }, data: { resourceType: 'rag', resourceId, operation: 'read' } })
    expect(grant.ok(), await grant.text()).toBeTruthy()
    await page.reload()
    await page.getByRole('button', { name: /^(适应画布|Fit view)$/ }).click()
    await page.getByTestId('studio-node-retrieve').dblclick()
    await expect(page.getByTestId('knowledge-panel')).toBeVisible()
    await page.getByTestId('parameter-topK').getByRole('spinbutton').fill('1')
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await saved(page, token, created)
    await page.getByTestId('node-details-view').getByRole('button', { name: '关闭', exact: true }).click()
    const success = await run(page, token, created, 'succeeded')
    expect(JSON.stringify(success)).toContain('sapphire-orbit')
    expect(JSON.stringify(success)).toContain('diagram-1')
    expect(JSON.stringify(success)).toContain('guide-1')
    expect((success.output as { documents: unknown[] }).documents).toHaveLength(1)
    if (provider === 'ragflow') {
      expect(success.output).toMatchObject({
        documents: [{ documentId: 'guide-1', title: 'Launch guide', metadata: { author: 'Support' } }],
        files: [{ documentId: 'guide-1', referenceId: 'diagram-1', uri: null }],
      })
    }
    await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
    for (const query of ['fixture-empty', 'fixture-error']) {
      value.definition.nodes.find(node => node.id === 'retrieve')!.parameters = { query: { kind: 'literal', value: query }, topK: 2 }
      await apply(page, value)
      const before = await saved(page, token, created)
      await page.getByTestId('studio-builder-panel').getByRole('button', { name: '关闭', exact: true }).click()
      const result = await run(page, token, created, query === 'fixture-error' ? 'failed' : 'succeeded')
      expect(result.source).toMatchObject({ kind: 'draft_revision', draftId: created.documentId, revision: before.revision })
      if (query === 'fixture-error') expect(result.error).toMatchObject({ outputs: { source_node_id: 'retrieve', code: expect.any(String), message: expect.any(String) } })
      else { expect(JSON.stringify(result)).toContain('"files":[]'); expect(JSON.stringify(result)).not.toContain('sapphire-orbit') }
      await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
    }
    expect(await count(page, token, created.id)).toBe(3)
    await info.attach('retrieval-result.json', { body: JSON.stringify(success), contentType: 'application/json' })
  })
}

test('P8 knowledge fixture Agent: all resource slots save and reopen without grants or execution', async ({ page }, info) => {
  const token = await login(page)
  const model = process.env.AGENTX_E2E_STUDIO_FIXTURE_ALIAS!
  const resources = JSON.parse(process.env.AGENTX_E2E_STUDIO_AGENT_RESOURCES!) as Record<string, string>
  await setModel(page, model)
  const created = await create(page, `P8 Agent slots ${Date.now()}`)
  const value = await document(page, token, created)
  const agent = value.definition.nodes.find(node => node.id === 'retrieve')!
  agent.type = 'agent'; agent.typeVersion = 2; agent.name = 'Configured Agent'
  agent.parameters = { systemPrompt: { kind: 'literal', value: 'Answer briefly' }, userQuestion: { kind: 'literal', value: 'Hello' }, sessionPolicy: { mode: 'invocation' } }
  agent.resourceReferences = []
  for (const [resourceType, bindingRole, operation, id] of [
    ['model', 'model', 'use', null], ['mcp_tool', 'mcp_tools', 'use', resources.tool],
    ['skill', 'skills', 'use', resources.skill], ['memory', 'long_term_memory', 'use', resources.memory],
    ['rag', 'knowledge', 'read', process.env.AGENTX_E2E_STUDIO_KNOWLEDGE_LIGHTRAG],
  ] as const) {
    const options = await read<{ items: Array<{ id: string; name: string; resourceVersionId: string | null }> }>(page, token, `/workflows/${created.id}/resource-options?draftId=${created.documentId}&resourceType=${resourceType}&operation=${operation}&pageSize=100`)
    const selected = options.items.find(item => id ? item.id === id : item.name === model)!
    expect(selected).toBeTruthy()
    agent.resourceReferences.push({ resourceType, bindingRole, operation, resourceId: selected.id, ...(selected.resourceVersionId ? { resourceVersionId: selected.resourceVersionId } : {}) })
  }
  const success = value.definition.nodes.find(node => node.protected)!
  success.parameters = { ...success.parameters as Record<string, unknown>, outputs: { answer: { kind: 'literal', value: 'Configured' } } }
  value.definition.end.outputs = { answer: { schema: { type: 'string' }, required: true, sensitive: false } }
  await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
  const [ackRequest] = await Promise.all([
    page.waitForRequest(request => request.method() === 'POST' && request.url().endsWith('/acknowledgements'), { timeout: 180_000 }),
    apply(page, value),
  ])
  const ack = ackRequest.postDataJSON() as components['schemas']['StudioEditorAcknowledgementV1']
  expect(ack.status).toBe('applied')
  const draft = await saved(page, token, created) as unknown as Draft
  // React Flow measures rendered dimensions after the ACK. Those additions
  // are expected; every acknowledged coordinate must survive storage exactly.
  expect(draft.editorDocument.nodeLayouts).toHaveLength(ack.snapshot.document.editorDocument.nodeLayouts.length)
  expect(draft.editorDocument.nodeLayouts).toMatchObject(ack.snapshot.document.editorDocument.nodeLayouts)
  const sessionPath = new URL(ackRequest.url()).pathname.replace('/api/v1', '').replace('/acknowledgements', '')
  const session = await read<components['schemas']['StudioBuilderSessionV1']>(page, token, sessionPath)
  expect(session.snapshot.documentHash).toBe(`sha256:v1:${createHash('sha256').update(canonicalDocument(session.snapshot.document)).digest('hex')}`)
  // A settled stream's final small frame must pass through the Web proxy
  // immediately, even when no more model output arrives to fill its buffers.
  const history = await allEntries<{ sequence: number }>(page, token, session.id)
  const lastSequence = history.at(-1)!.sequence
  const frame = await page.evaluate(async ({ path, token, after }) => {
    const response = await fetch(`/api/v1${path}/events?after=${after}`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() }, signal: AbortSignal.timeout(5000) })
    if (!response.ok) throw new Error(`SSE status ${response.status}`)
    const reader = response.body!.getReader()
    try { return new TextDecoder().decode((await reader.read()).value) } finally { await reader.cancel() }
  }, { path: sessionPath, token, after: lastSequence - 1 })
  expect(frame).toContain(`"sequence":${lastSequence}`)
  expect(draft.definition.nodes.find(node => node.id === agent.id)?.resourceReferences).toEqual(agent.resourceReferences)
  expect(await count(page, token, created.id)).toBe(0)
  await page.reload()
  await page.getByTestId(`studio-node-${agent.id}`).dblclick()
  await expect(page.getByTestId('agent-panel')).toBeVisible()
  await info.attach('fixture-agent-resources.json', { body: JSON.stringify(draft), contentType: 'application/json' })
  await page.getByTestId('node-details-view').getByRole('button', { name: '关闭', exact: true }).click()
  agent.resourceReferences = []
  value.editorDocument = draft.editorDocument
  await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
  await apply(page, value)
  const incomplete = await saved(page, token, created) as unknown as Draft
  expect(incomplete.definition.nodes.find(node => node.id === agent.id)?.resourceReferences).toEqual([])
  expect(await count(page, token, created.id)).toBe(0)
  await page.reload()
  await page.getByTestId(`studio-node-${agent.id}`).dblclick()
  await expect(page.getByTestId('agent-panel')).toBeVisible()
  const validation = await page.request.post(`/api/v1/workflows/${created.id}/drafts/${created.documentId}/validate`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() }, data: { definition: incomplete.definition, editorDocument: incomplete.editorDocument } })
  expect(validation.ok(), await validation.text()).toBeTruthy()
  expect(JSON.stringify(await validation.json())).toContain('BINDING_REQUIRED')
})

test('P8 knowledge live AI: selects named knowledge and model and reuses the normal End for errors', async ({ page }, info) => {
  const token = await login(page)
  const model = process.env.AGENTX_E2E_STUDIO_LIVE_ALIAS!
  await setModel(page, model)
  const created = await create(page, `P8 named resources ${Date.now()}`)
  await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
  await send(page, `创建一个完整工作流：开始→知识检索→模型→结束。知识库按名称选择“P8 Knowledge lightrag”，查询内容固定为“launch code”，Top K 为 2；业务模型按名称选择“${model}”，使用检索出的 text 作为问题生成简短回答。成功结束返回模型的 text，字段名 answer。请查询实际资源列表和节点契约后完成配置，不提供任何 ID 给你。缺授权时保留绑定并在总结列出，不运行、不授权、不发布。请检查现有结束节点是否可以接收错误；可以的话，把知识检索和模型的错误都接到这个结束节点的错误输入，保留它的成功输出，避免多余结束节点。`)
  await completed(page, true, 300_000)
  const draft = await saved(page, token, created) as unknown as Draft
  const knowledge = draft.definition.nodes.find(node => node.type === 'knowledge_retrieval')!
  const modelNode = draft.definition.nodes.find(node => node.type === 'model')!
  const expectedModel = await namedModel(page, token, created, model)
  expect(knowledge?.resourceReferences).toContainEqual(expect.objectContaining({ resourceType: 'rag', resourceId: process.env.AGENTX_E2E_STUDIO_KNOWLEDGE_LIGHTRAG, operation: 'read' }))
  expect(modelNode?.resourceReferences).toContainEqual(expect.objectContaining({ resourceType: 'model', operation: 'use', resourceId: expectedModel.id, resourceVersionId: expectedModel.resourceVersionId }))
  const errorExits = [knowledge, modelNode].map(node => draft.definition.connections.find(edge => edge.sourceNodeId === node.id && edge.sourceHandle === 'error'))
  expect(errorExits.every(edge => edge?.targetHandle === 'error')).toBeTruthy()
  const normalEnd = draft.definition.nodes.find(node => node.type === 'exit' && node.protected)!
  expect(draft.definition.nodes.filter(node => node.type === 'exit')).toHaveLength(1)
  expect(errorExits.every(edge => edge?.targetNodeId === normalEnd.id)).toBeTruthy()
  expect((normalEnd.parameters as { outputs: unknown }).outputs).toHaveProperty('answer')
  expect(Object.keys((normalEnd.parameters as { errorOutputs: object }).errorOutputs).length).toBeGreaterThan(0)
  expect(await count(page, token, created.id)).toBe(0)
  await expect(page.getByTestId('studio-builder-panel').getByTestId('markdown-content').last()).toContainText(/授权/)
  await info.attach('live-named-resources.json', { body: JSON.stringify(draft), contentType: 'application/json' })
})

test('P8 knowledge live AI: unmatched Agent resources remain empty and draft reopens', async ({ page }) => {
  const token = await login(page)
  await setModel(page, process.env.AGENTX_E2E_STUDIO_LIVE_ALIAS!)
  const created = await create(page, `P8 missing resources ${Date.now()}`)
  await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
  await send(page, '创建开始→Agent→结束。Agent 使用模型“missing-model-92731”、MCP工具“missing-orders-92731”、技能“missing-skill-92731”、长期记忆“missing-memory-92731”和知识库“missing-knowledge-92731”。按名称查询，找不到就不配置，不用其他资源替换，不提问。sessionPolicy 用 invocation，任务是回答你好。成功结束输出一个固定文本“待配置”。为 Agent 新建独立错误结束节点并连接 error 端口，不复用成功结束节点；最后逐项列出未找到的配置。不要运行。')
  await completed(page, true, 300_000)
  const draft = await saved(page, token, created) as unknown as Draft
  const agent = draft.definition.nodes.find(node => node.type === 'agent')!
  expect(agent).toBeTruthy()
  expect(agent.resourceReferences).toEqual([])
  expectErrorExit(draft, agent.id)
  expect(await count(page, token, created.id)).toBe(0)
  const summary = page.getByTestId('studio-builder-panel').getByTestId('markdown-content').last()
  for (const name of ['missing-model-92731', 'missing-orders-92731', 'missing-skill-92731', 'missing-memory-92731', 'missing-knowledge-92731']) await expect(summary).toContainText(name)
  await page.reload()
  await page.getByTestId(`studio-node-${agent.id}`).dblclick()
  await expect(page.getByTestId('agent-panel')).toBeVisible()
  const validate = await page.request.post(`/api/v1/workflows/${created.id}/drafts/${created.documentId}/validate`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() }, data: { definition: draft.definition, editorDocument: draft.editorDocument } })
  expect(validate.ok(), await validate.text()).toBeTruthy()
  expect(JSON.stringify(await validate.json())).toContain('BINDING_REQUIRED')
})

test('P8 knowledge live AI: binds named MCP, memory, skill and knowledge Agent attachments', async ({ page }, info) => {
  const token = await login(page)
  const model = process.env.AGENTX_E2E_STUDIO_LIVE_ALIAS!
  const resources = JSON.parse(process.env.AGENTX_E2E_STUDIO_AGENT_RESOURCES!) as Record<string, string>
  await setModel(page, model)
  const created = await create(page, `P8 Agent resources ${Date.now()}`)
  await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
  await send(page, `创建开始→Agent→结束。请按名称查询并配置：模型“${model}”、MCP 工具“echo”（来自 P8 MCP）、技能“P8 Skill”、长期记忆“P8 Memory”、知识库“P8 Knowledge lightrag”。Agent 会话使用 invocation，任务为基于知识回答你好；记忆和知识库只读。使用已发布资源版本，不要额外添加沙箱。成功结束返回固定文本“配置完成”，另外为 Agent 新建独立错误结束节点并连接 error 端口，不复用成功结束节点。缺授权则保留配置并总结，不运行、不授权。`)
  await completed(page, true, 300_000)
  const draft = await saved(page, token, created) as unknown as Draft
  const agent = draft.definition.nodes.find(node => node.type === 'agent')!
  expect(agent).toBeTruthy()
  expectErrorExit(draft, agent.id)
  const expectedModel = await namedModel(page, token, created, model)
  expect(agent.resourceReferences).toContainEqual(expect.objectContaining({ resourceType: 'model', bindingRole: 'model', resourceId: expectedModel.id, resourceVersionId: expectedModel.resourceVersionId }))
  for (const [resourceType, bindingRole, resourceId, operation] of [
    ['mcp_tool', 'mcp_tools', resources.tool, 'use'], ['skill', 'skills', resources.skill, 'use'],
    ['memory', 'long_term_memory', resources.memory, 'use'],
    ['rag', 'knowledge', process.env.AGENTX_E2E_STUDIO_KNOWLEDGE_LIGHTRAG, 'read'],
  ]) expect(agent.resourceReferences).toContainEqual(expect.objectContaining({ resourceType, bindingRole, resourceId, operation }))
  expect(agent.resourceReferences.find(ref => ref.resourceType === 'skill')?.resourceVersionId).toBe(resources.skillVersion)
  expect(agent.resourceReferences.find(ref => ref.resourceType === 'mcp_tool')?.resourceVersionId).toBeTruthy()
  expect(agent.resourceReferences).toHaveLength(5)
  expect(await count(page, token, created.id)).toBe(0)
  await page.reload()
  await page.getByTestId(`studio-node-${agent.id}`).dblclick()
  await expect(page.getByTestId('agent-panel')).toBeVisible()
  await info.attach('live-agent-resources.json', { body: JSON.stringify(draft), contentType: 'application/json' })
})


test('P8 recovered execution: failed history and full trace remain visible', async ({ page }) => {
  const target = process.env.AGENTX_E2E_RECOVERED_EXECUTION
  test.skip(!target, 'Recovery injection is orchestrated by pytest')
  const { workflowId, executionId } = JSON.parse(target!) as Record<string, string>
  await login(page)
  await page.goto(`/workflows/${workflowId}/versions/${await fixtureVersionId(page, workflowId)}/editor`)
  const select = page.getByRole('combobox', { name: /切换执行/ })
  await select.click()
  await page.getByRole('option', { name: /失败/ }).first().click()
  await page.getByRole('tab', { name: 'Trace', exact: true }).click()
  await expect(page.getByTestId('trace-workspace')).toBeVisible()
  await expect(page.getByTestId('trace-final-output')).toContainText('失败')
  await expect(page.getByTestId('execution-trace')).toContainText('Retrieve knowledge')
  await page.getByRole('tab', { name: '事件', exact: true }).click()
  await expect(page.getByTestId('execution-events')).toContainText('execution.failed')
  expect(executionId).toBeTruthy()
})

test('P8 knowledge multi-connection picker saves both queries and Agent knowledge slots', async ({ page }) => {
  const token = await login(page)
  await setModel(page, process.env.AGENTX_E2E_STUDIO_FIXTURE_ALIAS!)
  const created = await create(page, `P8 multiple sources ${Date.now()}`)
  const ids = [process.env.AGENTX_E2E_STUDIO_KNOWLEDGE_LIGHTRAG!, process.env.AGENTX_E2E_STUDIO_KNOWLEDGE_RAGFLOW!]
  for (const resourceId of ids) {
    const response = await page.request.post(`/api/v1/workflows/${created.id}/resource-authorizations`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key': crypto.randomUUID() }, data: { resourceType: 'rag', resourceId, operation: 'read' } })
    expect(response.ok(), await response.text()).toBeTruthy()
  }
  const value = await document(page, token, created, ids[0])
  await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
  await apply(page, value)
  await page.reload()
  await page.getByTestId('studio-node-retrieve').dblclick()
  await page.getByTestId('resource-selector-rag').getByRole('combobox').click()
  const list = page.getByRole('listbox')
  await expect(list.getByText('P8 Knowledge lightrag', { exact: true }).first()).toBeVisible()
  await expect(list.getByText('P8 Knowledge ragflow', { exact: true }).first()).toBeVisible()
  await list.getByRole('option', { name: /P8 Knowledge ragflow/ }).click()
  await page.getByRole('button', { name: '保存', exact: true }).click()
  let draft = await saved(page, token, created) as unknown as Draft
  expect(draft.definition.nodes.find(node => node.id === 'retrieve')!.resourceReferences.map(ref => ref.resourceId)).toEqual(ids)
  await page.reload()
  await page.getByTestId('studio-node-retrieve').dblclick()
  const panel = page.getByTestId('knowledge-panel')
  for (const name of ['P8 Knowledge lightrag', 'P8 Knowledge ragflow']) await expect(panel).toContainText(name)
  // The Agent registry also accepts multiple query resources in its knowledge slot.
  const agent = draft.definition.nodes.find(node => node.id === 'retrieve')!
  agent.type = 'agent'; agent.typeVersion = 2
  agent.parameters = { systemPrompt: { kind: 'literal', value: 'Search relevant project knowledge' }, userQuestion: { kind: 'literal', value: 'Launch code' }, sessionPolicy: { mode: 'invocation' } }
  for (const ref of agent.resourceReferences) ref.bindingRole = 'knowledge'
  const exit = draft.definition.nodes.find(node => node.protected)!
  exit.parameters = { outputs: { answer: { kind: 'literal', value: 'configured' } }, errorOutputs: {} }
  draft.definition.end.outputs = { answer: { schema: { type: 'string' }, required: true, sensitive: false } }
  draft.definition.end.error.outputs = {}
  await page.getByTestId('node-details-view').getByRole('button', { name: '关闭', exact: true }).click()
  await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
  await apply(page, { definition: draft.definition, editorDocument: draft.editorDocument })
  draft = await saved(page, token, created) as unknown as Draft
  expect(draft.definition.nodes.find(node => node.id === 'retrieve')!.resourceReferences).toHaveLength(2)
  await page.reload()
  await page.getByTestId('studio-node-retrieve').dblclick()
  for (const name of ['P8 Knowledge lightrag', 'P8 Knowledge ragflow']) await expect(page.getByTestId('agent-panel')).toContainText(name)
})
