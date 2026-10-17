import { fixtureVersionId, fixtureDraftId, fixtureDraftIdKnown } from '../draft-fixtures'
import { expect, type Locator, type Page, test } from '@playwright/test'

const password = 'agentx-e2e-admin-password'

type StudioDraft = {
  revision: number
  definition: {
    nodes: Array<{ id: string; key: string; type: string; name: string; protected?: boolean; parameters: Record<string, unknown> }>
    connections: Array<{ id: string; sourceNodeId: string; targetNodeId: string; targetHandle: string }>
    end: { completion: 'first_return' | 'all_complete'; outputs: Record<string, { schema: { type: string }; required: boolean }> }
  }
}

async function login(page: Page) {
  const bootstrapStatus = await page.request.get('/api/v1/bootstrap/status')
  if (!bootstrapStatus.ok()) throw new Error(`bootstrap status: ${bootstrapStatus.status()}`)
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

async function api<T>(page: Page, token: string, path: string): Promise<T> {
  const response = await page.request.get(`/api/v1${path}`, { headers: { Authorization: `Bearer ${token}`, 'Idempotency-Key':crypto.randomUUID() } })
  if (!response.ok()) throw new Error(`${path}: ${response.status()} ${await response.text()}`)
  return response.json() as Promise<T>
}

async function createWorkflow(page: Page, name: string) {
  await page.getByRole('link', { name: '工作流', exact: true }).click()
  await page.getByRole('button', { name: '新建工作流' }).click()
  const dialog = page.getByRole('dialog', { name: '新建工作流' })
  await dialog.getByLabel('工作流名称').fill(name)
  await dialog.getByLabel('描述').fill('Multi Exit Kubernetes E2E')
  await dialog.getByRole('button', { name: '保存' }).click()
  await expect(page).toHaveURL(/\/workflows\/[0-9a-f-]+$/)
  const workflowId = page.url().split('/').at(-1) as string
  await page.locator(`a[href="/workflows/${workflowId}/versions/${await fixtureVersionId(page, workflowId)}/editor"]`).click()
  await expect(page).toHaveURL(new RegExp(`/workflows/${workflowId}/versions/${await fixtureVersionId(page, workflowId)}/editor$`))
  return workflowId
}

async function connect(page: Page, source: Locator, sourceHandle: string, target: Locator, targetHandle: string) {
  const edges = page.locator('.react-flow__edge')
  const edgeCount = await edges.count()
  const from = source.locator(`.react-flow__handle.source[data-handleid="${sourceHandle}"]`)
  const to = target.locator(`.react-flow__handle.target[data-handleid="${targetHandle}"]`)
  for (let attempt = 0; attempt < 5; attempt += 1) {
    if (await edges.count() === edgeCount + 1) return
    await expect(from).toBeVisible()
    await expect(to).toBeVisible()
    const fromBox = await from.boundingBox()
    const toBox = await to.boundingBox()
    if (!fromBox || !toBox) throw new Error('Connection handle is not measurable')
    await page.mouse.move(fromBox.x + fromBox.width / 2, fromBox.y + fromBox.height / 2)
    await page.mouse.down()
    await page.mouse.move(fromBox.x + fromBox.width / 2 + 12, fromBox.y + fromBox.height / 2, { steps: 4 })
    await page.waitForTimeout(100)
    await page.mouse.move(toBox.x + toBox.width / 2, toBox.y + toBox.height / 2, { steps: 30 })
    await page.waitForTimeout(100)
    await page.mouse.up()
    await expect.poll(() => edges.count(), { timeout: 3_000 }).toBe(edgeCount + 1).catch(async () => { await page.keyboard.press('Escape') })
  }
  for (let attempt = 0; attempt < 2 && await edges.count() === edgeCount; attempt += 1) {
    await from.click({ force: true })
    await to.click({ force: true })
    await expect.poll(() => edges.count(), { timeout: 3_000 }).toBe(edgeCount + 1).catch(async () => { await page.keyboard.press('Escape') })
  }
  await expect(edges).toHaveCount(edgeCount + 1)
}

async function chooseInputsReference(page: Page, mapping: Locator) {
  await mapping.getByRole('textbox', { name: 'Value' }).click()
  const picker = page.getByTestId('reference-picker')
  await expect(picker).toBeVisible()
  const inputsRoot = picker.getByRole('button', { name: /输入|Inputs/ }).first()
  const toggle = inputsRoot.locator('[data-tree-toggle]')
  if (await toggle.count()) await toggle.click()
  await picker.getByRole('button', { name: /question/i }).first().click()
  await expect(picker).toBeHidden()
}

async function saveAndReadDraft(page: Page, token: string, workflowId: string) {
  const save = page.getByRole('button', { name: '保存', exact: true })
  const responsePromise = page.waitForResponse((response) => response.url().endsWith(`/api/v1/workflows/${workflowId}/drafts/${fixtureDraftIdKnown(workflowId)}`) && response.request().method() === 'PUT')
  await save.click()
  const response = await responsePromise
  if (!response.ok()) throw new Error(`save draft: ${response.status()} ${await response.text()}`)
  await expect(page.locator('header').getByText(/修订号 \d+ · 已保存/)).toBeVisible({ timeout: 30_000 })
  return api<StudioDraft>(page, token, `/workflows/${workflowId}/drafts/${await fixtureDraftId(page, workflowId)}`)
}

async function setStartQuestionInput(page: Page) {
  await page.getByTestId('workflow-start').click()
  const panel = page.getByTestId('start-panel')
  await expect(panel).toBeVisible()
  await panel.getByRole('button', { name: /添加字段|Add field/ }).first().click()
  const dialog = page.getByRole('dialog', { name: /添加字段|Add field/ })
  await dialog.getByLabel(/字段名|Field name/).fill('question')
  await dialog.getByLabel(/必填|Required/).check()
  await dialog.getByRole('button', { name: /保存|Save/ }).click()
  await panel.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
}

test.describe.configure({ mode: 'serial' })

test('multi exit workflow keeps one shared contract with per-node mappings', async ({ page }) => {
  const token = await login(page)
  const workflowName = `multi-exit-${Date.now()}`
  const workflowId = await createWorkflow(page, workflowName)

  await expect(page.getByTestId('exit-node-exit')).toBeVisible()
  await expect(page.getByTestId('exit-node-exit').locator('[data-testid="exit-protected"]')).toBeVisible()

  // the initial exit cannot be deleted
  await page.getByTestId('exit-node-exit').click()
  await page.keyboard.press('Delete')
  await expect(page.getByTestId('exit-node-exit')).toBeVisible()

  const initialEdge = page.locator('.react-flow__edge').first()
  await initialEdge.click({ force: true })
  await page.keyboard.press('Delete')
  await expect(page.locator('.react-flow__edge')).toHaveCount(0)

  // add a second, removable exit from the palette
  await page.getByTestId('palette-exit').click()
  await expect(page.getByTestId('exit-panel')).toBeVisible()
  await page.getByTestId('exit-panel').getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
  const secondExit = page.getByTestId('exit-node-exit_2')
  await expect(secondExit).toBeVisible()

  // two parallel branches, each terminating at its own exit
  await setStartQuestionInput(page)
  const start = page.getByTestId('workflow-start')
  const initialExit = page.getByTestId('exit-node-exit')
  async function addSet() {
    await expect(page.getByTestId('node-creator')).toBeVisible()
    await page.getByRole('textbox', { name: '搜索节点' }).fill('set')
    await page.getByTestId('palette-action-set').click()
    const selected = page.locator('.react-flow__node.selected').first()
    await expect(selected).toBeVisible()
    const testId = await selected.locator('[data-testid^="studio-node-"]').getAttribute('data-testid')
    expect(testId).toBeTruthy()
    const added = page.getByTestId(testId!)
    await page.getByTestId('node-details-view').getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
    return added
  }
  const firstSet = await addSet()
  const secondSet = await addSet()
  await connect(page, start, 'main', firstSet, 'main')
  await connect(page, start, 'main', secondSet, 'main')
  await connect(page, firstSet, 'main', initialExit, 'main')
  await connect(page, secondSet, 'main', secondExit, 'main')

  // shared contract declared once, mapped per exit
  await initialExit.click()
  const panel = page.getByTestId('exit-panel')
  await expect(panel).toBeVisible()
  await panel.getByRole('button', { name: /添加字段|Add field/ }).first().click()
  const dialog = page.getByRole('dialog', { name: /输出字段|Output field/ })
  await dialog.getByLabel(/输出名称|Output name/).fill('answer')
  await dialog.getByRole('button', { name: /保存|Save/ }).click()
  await expect(panel.getByTestId('exit-mapping-answer')).toBeVisible()
  await chooseInputsReference(page, panel.getByTestId('exit-mapping-answer'))
  await panel.getByTestId('completion-all_complete').click()
  await panel.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()

  await secondExit.click()
  const secondPanel = page.getByTestId('exit-panel')
  await expect(secondPanel).toBeVisible()
  await expect(secondPanel.getByTestId('exit-mapping-answer')).toBeVisible()
  await secondPanel.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()

  const { definition } = await saveAndReadDraft(page, token, workflowId)
  const exitNodes = definition.nodes.filter((node) => node.type === 'exit')
  const primaryExit = exitNodes.find((node) => node.key === 'exit')
  const manualExit = exitNodes.find((node) => node.key === 'exit_2')
  expect(exitNodes).toHaveLength(2)
  expect(exitNodes.map((node) => node.protected ?? false).sort()).toEqual([false, true])
  expect(primaryExit?.parameters.outputs).toHaveProperty('answer')
  expect(manualExit?.parameters.outputs ?? {}).not.toHaveProperty('answer')
  const exitIds = new Set(exitNodes.map((node) => node.id))
  expect(definition.connections.some((connection) => exitIds.has(connection.targetNodeId) && connection.targetHandle === 'main')).toBe(true)
  expect(definition.end.completion).toBe('all_complete')
  expect(definition.end.outputs.answer).toMatchObject({ schema: { type: 'string' }, required: false })

  // Both exits arrive, but only the first maps the optional field. The output
  // keeps one slot per reached Exit in Definition order.
  const runResponse = page.waitForResponse((response) => response.url().includes('/debug-executions') && response.request().method() === 'POST')
  await page.locator('header').getByRole('button', { name: '运行', exact: true }).click()
  const parameters = page.getByRole('dialog', { name: /运行工作流|调试输入|Run workflow|Debug input/ })
  if (await parameters.waitFor({ state: 'visible', timeout: 2_000 }).then(() => true).catch(() => false)) {
    await parameters.getByLabel(/Question|问题/).fill('first-branch-answer')
    await parameters.getByRole('button', { name: /运行|Run/ }).click()
  }
  const started = await runResponse
  const executionId = ((await started.json()) as { executionId: string }).executionId
  await expect.poll(async () => (await api<{ status: string }>(page, token, `/executions/${executionId}`)).status, { timeout: 120_000 }).toBe('succeeded')
  const execution = await api<{ output: { answer: Array<string | null> } }>(page, token, `/executions/${executionId}`)
  expect(execution.output.answer).toEqual(['first-branch-answer', null])

  // the manually added exit can be removed; the protected one cannot
  await secondExit.click()
  await page.keyboard.press('Delete')
  await expect(secondExit).toHaveCount(0)
  await expect(initialExit).toBeVisible()

  // Removing the manual Exit remains undoable/saveable; the protected Exit remains.
  await saveAndReadDraft(page, token, workflowId)
})
