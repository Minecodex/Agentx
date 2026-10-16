import { expect, type Page } from '@playwright/test'
import { readFileSync } from 'node:fs'
import type { components } from '../../../src/web/src/shared/api/generated'
import { test, completed, count, create, createVersion, login, read, rename, saved, send, setModel, type Created } from '../studio-builder-support'

type Document = components['schemas']['StudioEditorSnapshotV1']['document']
type DetailedDraft = Document & { revision: number }
const cases = JSON.parse(readFileSync(new URL('../../../src/web/src/features/workflow-designer/testing/ai-node-parameters.fixture.json', import.meta.url), 'utf8')) as Record<string, Record<string, unknown>>
const types = Object.keys(cases)
const input = { type: 'object', properties: { question: { type: 'string' }, rows: { type: 'array', items: { type: 'object', properties: { question: { type: 'string' } }, required: ['question'] } } }, required: ['question', 'rows'], additionalProperties: false }
const ref = (name: string) => ({ kind: 'reference', selector: { namespace: 'inputs', path: [name] } })
const definitionNode = (type: string, parameters: Record<string, unknown>, id = 'inspect_node') => ({ id, key: id, name: `Inspect ${type}`, type, typeVersion: type === 'agent' ? 2 : 1, parameters, settings: { alwaysOutputData: false, executeOnce: false, maxTries: 1, retryOnFail: false, timeoutMs: null, waitBetweenTriesMs: 0 }, contextWrites: [], resourceReferences: [], disabled: false, protected: false })
const edge = (sourceNodeId: string, targetNodeId: string, sourceHandle = 'main', targetHandle = 'main') => ({ id: `${sourceNodeId}-${sourceHandle}-${targetNodeId}`, sourceNodeId, targetNodeId, sourceHandle, targetHandle, order: 0 })
const nodeCard = (page: Page, id: string, type: string) => type === 'loop_over_items' ? page.getByTestId(`studio-loop-${id}`).locator('header') : page.getByTestId(`studio-node-${id}`)

async function fixtureDocument(page: Page, token: string, type: string, created: Created): Promise<Document> {
  const draft = await read<DetailedDraft>(page, token, `/workflows/${created.id}/drafts/${created.createdDraftId}`)
  const document: Document = { definition: structuredClone(draft.definition), editorDocument: structuredClone(draft.editorDocument) }
  const parameters = structuredClone(cases[type])
  if (type === 'approval') parameters.candidateUserId = (await read<{ id: string }>(page, token, '/auth/me')).id
  if (type === 'sub_workflow') {
    const child = await create(page, `Inspector child ${Date.now()}`)
    const value = await read<DetailedDraft>(page, token, `/workflows/${child.id}/drafts/${child.createdDraftId}`)
    value.definition.start.inputs = { type: 'object', properties: { question: { type: 'string' }, nested: { type: 'array', items: { type: 'string' } } }, additionalProperties: false }
    const response = await page.request.put(`/api/v1/workflows/${child.id}/drafts/${child.createdDraftId}`, { headers: { Authorization: `Bearer ${token}` }, data: { expectedRevision: value.revision, definition: value.definition, editorDocument: value.editorDocument } })
    expect(response.ok(), await response.text()).toBeTruthy()
    parameters.workflowVersionId = (await createVersion(page, child)).id
    await page.goto(`/workflows/${created.id}/drafts/${created.createdDraftId}/editor`)
    await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  }
  const node: Document['definition']['nodes'][number] = definitionNode(type, parameters)
  if (type === 'model' || type === 'agent') {
    const aliases = await read<{ items: Array<{ id: string; alias: string; deploymentId: string }> }>(page, token, '/models/aliases?pageSize=100')
    const model = aliases.items.find(item => item.alias === process.env.AGENTX_E2E_STUDIO_LIVE_ALIAS)!
    node.resourceReferences = [{ bindingRole: 'model', resourceType: 'model', resourceId: model.id, resourceVersionId: model.deploymentId, operation: 'use' }]
  }
  document.definition.start.inputs = input
  document.definition.end.outputs = { reply: { schema: { type: 'string' }, required: true, sensitive: false } }
  const exit = document.definition.nodes.find(item => item.type === 'exit')!
  exit.parameters = { outputs: { reply: type === 'model' ? { kind: 'reference', selector: { namespace: 'outputs', sourceNodeId: node.id, path: ['text'] } } : ref('question') }, errorOutputs: {} }
  document.definition.nodes.push(node)
  document.definition.connections = [edge('__start__', node.id), edge(node.id, exit.id, type === 'if' ? 'case:match' : type === 'approval' ? 'decision:approve' : 'main')]
  if (type === 'if') document.definition.connections.push(edge(node.id, exit.id, 'else'))
  if (type === 'loop_over_items') document.definition.nodes.push({ ...definitionNode('set', { values: { kind: 'object', fields: { question: ref('question') } }, keepOnlySet: true }, 'loop_body'), parentId: node.id })
  return document
}

async function inspectAndReopen(page: Page, token: string, created: Created, nodeId: string, type: string) {
  const before = await saved(page, token, created) as unknown as DetailedDraft
  await page.getByTestId('studio-builder-panel').getByRole('button', { name: '关闭', exact: true }).click()
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/ }).click()
  await nodeCard(page, nodeId, type).click()
  await expect(page.getByTestId('node-details-view')).toBeVisible()
  if (type === 'code') await codeWorkerReady(page)
  await page.getByTestId('node-details-view').getByRole('button', { name: '关闭', exact: true }).click()
  await rename(page, `Edited ${type}`, nodeId, type === 'loop_over_items' ? 'loop' : 'node')
  const edited = await saved(page, token, created) as unknown as DetailedDraft
  expect(edited.definition.nodes.find(node => node.id === nodeId)?.parameters).toEqual(before.definition.nodes.find(node => node.id === nodeId)?.parameters)
  await page.reload()
  await expect(nodeCard(page, nodeId, type)).toContainText(`Edited ${type}`)
  await nodeCard(page, nodeId, type).dblclick()
  await expect(page.getByTestId('node-details-view')).toBeVisible()
  if (type === 'code') await codeWorkerReady(page)
  await page.getByTestId('node-details-view').getByRole('button', { name: '关闭', exact: true }).click()
  await page.locator('.react-flow__node-exit').first().dblclick()
  await expect(page.getByTestId('exit-mapping-reply')).toBeVisible()
  expect(await count(page, token, created.id)).toBe(0)
}

async function codeWorkerReady(page: Page) {
  await expect(page.getByTestId('node-details-view').locator('.monaco-editor').first()).toBeVisible()
  await expect.poll(() => page.workers().some(worker => /\/ts\.worker-[^/]+\.js/.test(worker.url()))).toBeTruthy()
  const worker = page.workers().find(worker => /\/ts\.worker-[^/]+\.js/.test(worker.url()))!
  expect(await worker.evaluate(() => typeof self.onmessage)).toBe('function')
}

for (const type of types) {
  test(`P8 inspector fixture ${type}: compact bindings, Markdown, layout, click/edit/reopen`, async ({ page }, info) => {
    const errors: string[] = []
    page.on('pageerror', error => errors.push(error.message))
    const token = await login(page)
    await setModel(page, process.env.AGENTX_E2E_STUDIO_FIXTURE_ALIAS!)
    const created = await create(page, `Inspector fixture ${type} ${Date.now()}`)
    const document = await fixtureDocument(page, token, type, created)
    await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
    let drop = type === 'model'
    await page.route('**/studio-builder/sessions/*/acknowledgements', async route => {
      if (drop) { drop = false; await route.abort('connectionreset') } else await route.continue()
    })
    await send(page, `P8FIX: inspector=${JSON.stringify(document)}`)
    await completed(page)
    const markdown = page.getByTestId('studio-builder-panel').getByTestId('markdown-content').last()
    await expect(markdown.getByRole('heading', { name: '编辑完成' })).toBeVisible()
    await expect(markdown.locator('strong')).toHaveText('节点')
    await expect(markdown.locator('pre code')).toHaveText('{"executed":false}')
    const after = await saved(page, token, created) as unknown as DetailedDraft
    const start = after.editorDocument.boundaryLayouts!.find(item => item.boundary === 'start')!
    const target = after.editorDocument.nodeLayouts!.find(item => item.nodeId === 'inspect_node')!
    expect(target.x - start.x - 240).toBeGreaterThanOrEqual(159)
    if (type === 'loop_over_items') {
      const exit = after.editorDocument.nodeLayouts!.find(item => item.nodeId === 'exit')!
      expect(exit.x - target.x - target.width!).toBeCloseTo(160)
    }
    await page.getByRole('button', { name: /^(适应画布|Fit view)$/ }).click()
    await markdown.scrollIntoViewIfNeeded()
    await page.screenshot({ path: info.outputPath(`${type}-markdown-layout.png`) })
    const malformed = structuredClone(after)
    const bad = malformed.definition.nodes.find(item => item.id === 'inspect_node')!
    bad.parameters = { ...bad.parameters as Record<string, unknown>, unknownInspectorField: true }
    const appliedCount = await page.getByText('已应用一个编辑事务', { exact: true }).count()
    await send(page, `P8FIX: inspector=${JSON.stringify({ definition: malformed.definition, editorDocument: malformed.editorDocument })}`)
    await completed(page, false)
    expect(await page.getByText('已应用一个编辑事务', { exact: true }).count()).toBe(appliedCount)
    expect((await saved(page, token, created) as unknown as DetailedDraft).definition).toEqual(after.definition)
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await expect(nodeCard(page, 'inspect_node', type)).toHaveCount(0)
    await page.getByRole('button', { name: '重做', exact: true }).click()
    await expect(nodeCard(page, 'inspect_node', type)).toBeVisible()
    await inspectAndReopen(page, token, created, 'inspect_node', type)
    expect(errors).toEqual([])
  })

  test(`P8 inspector real GLM ${type}: AI creation and editable node`, async ({ page }, info) => {
    const errors: string[] = []
    page.on('pageerror', error => errors.push(error.message))
    const token = await login(page)
    await setModel(page, process.env.AGENTX_E2E_STUDIO_LIVE_ALIAS!)
    const created = await create(page, `Inspector GLM ${type} ${Date.now()}`)
    const desired = await fixtureDocument(page, token, type, created)
    await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
    await send(page, `在当前画布创建 ${type} 节点。请先读取画布和该节点的真实 Manifest，再实现以下设计。所有需要的信息都在下列 Definition 中，可以按需修正参数使其符合契约，保留受保护的 Exit。不要移除 reply 输出。不要运行、测试、授权或创建版本，不要提问；缺少运行资源时保留可编辑草稿并在结果说明。只做编辑和静态校验。不要计算坐标，使用画布自动布局。请用 Markdown 总结结果。目标设计：${JSON.stringify(desired.definition)}`)
    await completed(page)
    const result = await saved(page, token, created) as unknown as DetailedDraft
    const node = result.definition.nodes.find(item => item.type === type && !item.parentId)
    expect(node, 'The real model must actually create the requested node').toBeDefined()
    await info.attach(`${type}-definition.json`, { body: JSON.stringify(result), contentType: 'application/json' })
    await inspectAndReopen(page, token, created, node!.id, type)
    expect(errors).toEqual([])
    await page.screenshot({ path: info.outputPath(`${type}-reopened.png`) })
  })
}
