import { expect } from '@playwright/test'
import { test, login, setModel, create, count, send, completed, saved, read } from '../studio-builder-support'

test('P8-E2E-01/02/11 Fixture: UI model setting, creation, atomic Undo, ACK loss and reconnect', async ({ page }, testInfo) => {
  const alias = process.env.AGENTX_E2E_STUDIO_FIXTURE_ALIAS
  if (!alias) throw new Error('Studio fixture alias is required')
  const token = await login(page)
  const connectionChecks: string[] = []
  page.on('request', request => { if (/models.*(?:test-connection|\/check|\/test)$/.test(request.url())) connectionChecks.push(request.url()) })
  await setModel(page, alias)
  expect(connectionChecks).toEqual([])
  const created = await create(page, `P8 Fixture ${Date.now()}`)
  expect(await count(page, token, created.id)).toBe(0)
  await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
  await send(page, 'P8FIX: label=Initial')
  await completed(page)
  expect(await count(page, token, created.id)).toBe(0)
  const initial = await saved(page, token, created)
  expect(initial.definition.nodes.some(node => node.id === 'p8-decision' && node.name === 'Initial')).toBeTruthy()
  let dropped = false
  await page.route('**/studio-builder/sessions/*/acknowledgements', async route => {
    if (!dropped) { dropped = true; await route.abort('connectionreset') } else await route.continue()
  })
  await send(page, 'P8FIX: label=Changed')
  await completed(page)
  await expect(page.getByTestId('studio-node-p8-decision')).toContainText('Changed')
  expect(dropped).toBeTruthy()
  expect(await count(page, token, created.id)).toBe(0)
  await page.getByRole('button', { name: '撤销', exact: true }).click()
  await expect(page.getByTestId('studio-node-p8-decision')).toContainText('Initial')
  await page.getByRole('button', { name: '重做', exact: true }).click()
  await expect(page.getByTestId('studio-node-p8-decision')).toContainText('Changed')
  await saved(page, token, created)
  await page.reload()
  await expect(page.getByTestId('studio-node-p8-decision')).toContainText('Changed')
  await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
  await expect(page.getByTestId('studio-builder-panel').getByText('已应用一个编辑事务', { exact: true })).toHaveCount(2)
  expect(await count(page, token, created.id)).toBe(0)
  await page.screenshot({ path: testInfo.outputPath('fixture-reconnect.png'), fullPage: true })
})

test('P8-E2E-01/02/14 Real glm-5.3-flash: create, local edit, save/reopen, Undo and manual run', async ({ page }, testInfo) => {
  const alias = process.env.AGENTX_E2E_STUDIO_LIVE_ALIAS
  if (!alias) throw new Error('Real glm-5.3-flash alias is required')
  const token = await login(page)
  await setModel(page, alias)
  const created = await create(page, `P8 GLM ${Date.now()}`)
  await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
  await send(page, '创建一个无需外部资源的简单工作流：保留受保护的 Exit，在 Start 与 Exit 之间添加 IF 节点，名称为 GLM判断。条件使用两个字面量 true 相等，分支和 else 都连到 Exit。请使用 node_catalog 查看 IF 的真实参数和端口，case 端口格式为 case:<id>。只进行编辑和静态检查，不运行、不测试、不创建版本。直接完成，无需提问。')
  await completed(page)
  expect(await count(page, token, created.id)).toBe(0)
  const first = await saved(page, token, created)
  const decision = first.definition.nodes.find(node => node.type === 'if')
  expect(decision?.name).toBe('GLM判断')
  const unrelated = first.definition.nodes.filter(node => node.id !== decision?.id)
  await send(page, '只把刚创建的 IF 节点名称改为 GLM复核，其余节点、连接、参数和布局保持原样。完成后静态校验，不运行。')
  await completed(page)
  const second = await saved(page, token, created)
  expect(second.definition.nodes.find(node => node.id === decision?.id)?.name).toBe('GLM复核')
  expect(second.definition.nodes.filter(node => node.id !== decision?.id)).toEqual(unrelated)
  expect(second.definition.connections).toEqual(first.definition.connections)
  expect(await count(page, token, created.id)).toBe(0)
  await page.getByRole('button', { name: '撤销', exact: true }).click()
  await expect(page.getByTestId(`studio-node-${decision?.id}`)).toContainText('GLM判断')
  await page.getByRole('button', { name: '重做', exact: true }).click()
  await saved(page, token, created)
  await page.reload()
  await expect(page.getByTestId(`studio-node-${decision?.id}`)).toContainText('GLM复核')
  await page.getByRole('button', { name: 'AI 创作', exact: true }).click()
  let session = ''
  page.on('request', request => {
    const match = request.url().match(/\/studio-builder\/sessions\/([^/]+)\/runs$/)
    if (match) session = match[1]!
  })
  for (let index = 0; index < 4; index += 1) {
    await send(page, `只把 IF 节点名称改为 GLM压缩${index}，保留其他节点、配置、连接和布局。完成静态校验，不运行、不提问。以下是重复的设计约束，无需逐项处理：${'Keep this Draft and its protected Exit. Never execute nodes, test, save, publish or grant resources. '.repeat(590)}\n当前目标只有重命名为 GLM压缩${index}。`)
    await completed(page)
    const current = await saved(page, token, created)
    expect(current.definition.nodes.find(node => node.id === decision?.id)?.name).toBe(`GLM压缩${index}`)
    expect(await count(page, token, created.id)).toBe(0)
  }
  let cursor = 0
  let compacted = false
  for (;;) {
    const entries = await read<{items: Array<{sequence: number; payload: {type: string}}>; nextCursor: number}>(page, token, `/studio-builder/sessions/${session}/entries?after=${cursor}`)
    compacted ||= entries.items.some(entry => entry.payload.type === 'compaction_completed')
    if (entries.items.length < 200) break
    cursor = entries.nextCursor
  }
  expect(compacted, 'The real provider must settle an actual compaction, not only normal edit calls').toBeTruthy()
  const accepted = page.waitForResponse(value => value.url().endsWith(`/drafts/${created.documentId}/debug-executions`) && value.request().method() === 'POST')
  await page.getByRole('button', { name: '运行', exact: true }).first().click()
  const response = await accepted
  expect(response.status(), await response.text()).toBe(202)
  const execution = await response.json() as { executionId: string }
  await expect.poll(async () => (await read<{ status: string }>(page, token, `/executions/${execution.executionId}`)).status, { timeout: 120_000 }).toBe('succeeded')
  const result = await read<{ source: { kind: string; draftId: string; revision: number }; workflowVersionId: string | null }>(page, token, `/executions/${execution.executionId}`)
  expect(result.source).toMatchObject({ kind: 'draft_revision', draftId: created.documentId })
  expect(result.workflowVersionId).toBeNull()
  expect(await count(page, token, created.id)).toBe(1)
  await page.screenshot({ path: testInfo.outputPath('glm-manual-run.png'), fullPage: true })
  await testInfo.attach('workflow-and-execution.json', { body: JSON.stringify({ workflowId: created.id, draftId: created.documentId, executionId: execution.executionId, source: result.source }), contentType: 'application/json' })
})
