import { fixtureVersionId, fixtureDraftId, fixtureDraftIdKnown } from '../draft-fixtures'
import { expect, type Locator, type Page, test } from '@playwright/test'
import { readFile } from 'node:fs/promises'

const password = 'agentx-e2e-admin-password'
const packagePath = process.env.AGENTX_PLUGIN_VARIANT_PATH ?? ''
const packageId = process.env.AGENTX_PLUGIN_VARIANT_PACKAGE_ID ?? 'acme/ai-variant'
const nodeType = process.env.AGENTX_PLUGIN_VARIANT_NODE_TYPE ?? 'acme.ai_variant'
const displayName = process.env.AGENTX_PLUGIN_VARIANT_DISPLAY_NAME ?? 'AI Variant Mapper'

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill(password)
  const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/auth/login'))
  await page.getByRole('button', { name: '登录' }).click()
  return ((await (await response).json()) as { accessToken: string }).accessToken
}

async function api<T>(page: Page, token: string, path: string): Promise<T> {
  const response = await page.request.get(`/api/v1${path}`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })
  if (!response.ok()) throw new Error(`${path}: ${response.status()} ${await response.text()}`)
  return response.json() as Promise<T>
}

async function connect(page: Page, source: Locator, sourceHandle: string, target: Locator, targetHandle: string) {
  const before = await page.locator('.react-flow__edge').count()
  const edges = page.locator('.react-flow__edge')
  const from = source.locator(`.react-flow__handle.source[data-handleid="${sourceHandle}"]`)
  const to = target.locator(`.react-flow__handle.target[data-handleid="${targetHandle}"]`)
  for (let attempt = 0; attempt < 5; attempt += 1) {
    if (await edges.count() === before + 1) return
    await expect(from).toBeVisible()
    await expect(to).toBeVisible()
    const fromBox = await from.boundingBox(); const toBox = await to.boundingBox()
    if (!fromBox || !toBox) throw new Error('Variant connection handle is not measurable')
    await page.mouse.move(fromBox.x + fromBox.width / 2, fromBox.y + fromBox.height / 2)
    await page.mouse.down()
    await page.mouse.move(fromBox.x + fromBox.width / 2 + 12, fromBox.y + fromBox.height / 2, { steps: 4 })
    await page.waitForTimeout(100)
    await page.mouse.move(toBox.x + toBox.width / 2, toBox.y + toBox.height / 2, { steps: 30 })
    await page.waitForTimeout(100)
    await page.mouse.up()
    await expect.poll(() => edges.count(), { timeout: 3_000 }).toBe(before + 1).catch(async () => { await page.keyboard.press('Escape') })
  }
  for (let attempt = 0; attempt < 2 && await edges.count() === before; attempt += 1) {
    await from.click({ force: true })
    await to.click({ force: true })
    await expect.poll(() => edges.count(), { timeout: 3_000 }).toBe(before + 1).catch(async () => { await page.keyboard.press('Escape') })
  }
  await expect(edges).toHaveCount(before + 1)
}

test('imports and executes the independently developed template variant', async ({ page }) => {
  test.skip(!packagePath, 'AGENTX_PLUGIN_VARIANT_PATH is required')
  const token = await login(page)
  await page.getByRole('link', { name: '画布插件', exact: true }).click()
  const existing = (await api<{ items: Array<{ packageId: string }> }>(page, token, '/canvas-plugins?pageSize=100')).items.some((item) => item.packageId === packageId)
  if (!existing) {
    await page.getByRole('button', { name: '导入插件' }).click()
    const dialog = page.getByRole('dialog', { name: '导入画布插件' })
    await dialog.locator('input[type=file]').setInputFiles({ name: 'ai-variant.agentx-plugin', mimeType: 'application/zip', buffer: await readFile(packagePath) })
    await expect(dialog.getByText(`${packageId}@1.1.0`)).toBeVisible()
    await dialog.getByRole('button', { name: '确认导入' }).click()
  }
  await expect(page.getByText(displayName, { exact: true }).first()).toBeVisible()

  const created = await page.request.post('/api/v1/workflows', { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() }, data: { name: `Template Variant ${Date.now()}`, visibility: 'company' } })
  expect(created.ok(), await created.text()).toBeTruthy()
  const workflow = await created.json() as { id: string }
  await page.goto(`/workflows/${workflow.id}/versions/${await fixtureVersionId(page, workflow.id)}/editor`)
  const draft = await api<{ definition: { connections: Array<{ id: string }> } }>(page, token, `/workflows/${workflow.id}/drafts/${await fixtureDraftId(page, workflow.id)}`)
  const edge = page.locator(`.react-flow__edge[data-testid="rf__edge-${draft.definition.connections[0].id}"]`)
  const toolbar = page.locator(`.studio-edge-toolbar[data-edge-id="${draft.definition.connections[0].id}"]`)
  await edge.click({ force: true })
  await expect(toolbar).toHaveCSS('opacity', '1')
  await toolbar.getByRole('button', { name: '删除连线' }).dispatchEvent('click')
  await page.getByRole('textbox', { name: '搜索节点' }).fill(displayName)
  await page.getByTestId(`palette-action-${nodeType}`).click()
  const inspector = page.getByTestId('node-details-view')
  await inspector.getByLabel('Label').fill('variant')
  await expect(inspector.getByLabel('Search provider')).toHaveCount(0)
  await inspector.getByLabel('Metadata output').check()
  const plugin = page.locator('.react-flow__node-manifest').filter({ hasText: displayName })
  await expect(plugin.locator('.react-flow__handle.source[data-handleid="metadata"]')).toBeVisible()
  await inspector.getByRole('button', { name: '关闭', exact: true }).first().click()
  await page.getByRole('button', { name: '适应画布', exact: true }).click({ force: true })
  await connect(page, page.getByTestId('workflow-start'), 'main', plugin, 'main')
  await connect(page, plugin, 'main', page.getByTestId('exit-node-exit'), 'main')
  const saved = page.waitForResponse((value) => value.url().endsWith(`/api/v1/workflows/${workflow.id}/drafts/${fixtureDraftIdKnown(workflow.id)}`) && value.request().method() === 'PUT')
  await page.locator('header').getByRole('button', { name: '保存', exact: true }).click()
  expect((await saved).ok()).toBeTruthy()
  const execution = page.waitForResponse((value) => value.url().includes('/debug-executions') && value.request().method() === 'POST')
  await page.locator('header').getByRole('button', { name: '运行', exact: true }).click()
  const runDialog = page.getByRole('dialog', { name: /运行工作流|调试输入/ })
  if (await runDialog.isVisible().catch(() => false)) await runDialog.getByRole('button', { name: /开始运行|运行/ }).click()
  const accepted = await execution
  expect(accepted.status()).toBe(202)
  const executionId = ((await accepted.json()) as { executionId: string }).executionId
  await expect.poll(async () => (await api<{ status: string }>(page, token, `/executions/${executionId}`)).status, { timeout: 120_000 }).toBe('succeeded')
  const runs = await api<{ items: Array<{ nodeType: string; output?: { metadata?: Array<{ json: { mapped: number } }> } }> }>(page, token, `/executions/${executionId}/nodes`)
  expect(runs.items.find((item) => item.nodeType === nodeType)?.output?.metadata?.[0].json.mapped).toBe(1)
  await expect.poll(async () => (await api<{ spans: Array<{ spanKind: string }> }>(page, token, `/executions/${executionId}/trace?limit=100`)).spans.some((span) => span.spanKind === 'plugin_operation'), { timeout: 30_000 }).toBeTruthy()
})
