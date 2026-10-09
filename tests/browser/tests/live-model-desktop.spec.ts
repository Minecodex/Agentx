import { expect, test, type Page } from '@playwright/test'
import { writeFile } from 'node:fs/promises'
import { useRuntimePortForward } from './playground-helpers'

async function login(page: Page) {
  await useRuntimePortForward(page)
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill('agentx-e2e-admin-password')
  const response = page.waitForResponse((item) => item.url().endsWith('/api/v1/auth/login') && item.request().method() === 'POST')
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await expect(page).toHaveURL(/\/$/)
  return ((await (await response).json()) as { accessToken: string }).accessToken
}

async function openConversation(page: Page) {
  const appId = process.env.AGENTX_E2E_LIVE_APP_ID
  if (!appId) throw new Error('Real Kimi application is required')
  await login(page)
  await page.goto(`/playground?applicationId=${appId}&mode=conversation`)
  await page.getByRole('button', { name: /新建会话/ }).click()
  await expect(page.locator('textarea')).toBeEnabled()
}

test('real Kimi text appears incrementally and survives a browser refresh', async ({ page }, testInfo) => {
  await openConversation(page)
  const question = `UI水星蓝桥-${Date.now()}。请分15条说明如何设计可靠的工作流系统,每条约30字。回答以UI水星蓝桥开头。`
  await page.locator('textarea').fill(question)
  const accepted = page.waitForResponse((response) => response.url().includes('/messages') && response.request().method() === 'POST')
  await page.getByRole('button', { name: '发送', exact: true }).click()
  expect((await accepted).status()).toBe(202)
  const invocationId = ((await (await accepted).json()) as { id: string }).id
  const pulse = page.locator('span.animate-pulse')
  await expect(pulse).toBeVisible({ timeout: 120_000 })
  const partial = await pulse.locator('..').innerText()
  expect(partial.length).toBeGreaterThan(0)
  await page.screenshot({ path: testInfo.outputPath('stream-in-progress.png'), fullPage: true })
  await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0, { timeout: 180_000 })
  const answer = page.locator('article').filter({ has: page.getByRole('link', { name: /Trace/ }) }).last()
  await expect(answer).toContainText('UI水星蓝桥')
  const finalText = await answer.locator('.whitespace-pre-wrap').innerText()
  expect(finalText.length).toBeGreaterThan(partial.length)
  await page.screenshot({ path: testInfo.outputPath('stream-completed.png'), fullPage: true })
  await page.reload()
  await expect(page.locator('article').getByText(question, { exact: true })).toBeVisible({ timeout: 30_000 })
  await expect(page.locator('article').last()).toContainText(finalText)
  await page.screenshot({ path: testInfo.outputPath('history-after-refresh.png'), fullPage: true })
  await writeFile(testInfo.outputPath('stream-results.json'), JSON.stringify({ invocationId, partialCharacters: partial.length, finalCharacters: finalText.length, refreshRestored: true }, null, 2))
  await page.getByRole('link', { name: /Trace/ }).last().click()
  await expect(page).toHaveURL(/\/executions\/[a-f0-9-]+$/)
  await page.getByRole('tab', { name: '高级瀑布', exact: true }).click()
  await expect(page.getByRole('treegrid', { name: 'Trace 层级瀑布' })).toBeVisible()
  await page.screenshot({ path: testInfo.outputPath('real-model-trace.png'), fullPage: true })
})

test('stop cancels a real Kimi generation after text has arrived', async ({ page }, testInfo) => {
  await openConversation(page)
  await page.locator('textarea').fill('请写5000字中文工作流设计教程,从第一章开始。')
  await page.getByRole('button', { name: '发送', exact: true }).click()
  await expect(page.locator('span.animate-pulse')).toBeVisible({ timeout: 120_000 })
  const cancelled = page.waitForResponse((response) => response.url().endsWith('/cancel') && response.request().method() === 'POST')
  await page.getByRole('button', { name: '停止', exact: true }).click()
  expect([200, 202]).toContain((await cancelled).status())
  await expect(page.getByText('已取消', { exact: true })).toBeVisible({ timeout: 60_000 })
  await expect(page.locator('textarea')).toBeEnabled()
  await page.screenshot({ path: testInfo.outputPath('cancelled-generation.png'), fullPage: true })
})

test('real V1/V2 Judge reports show missing cases and open Judge Trace', async ({ page }, testInfo) => {
  await login(page)
  const baseline = process.env.AGENTX_E2E_COMPARE_BASELINE
  const candidate = process.env.AGENTX_E2E_COMPARE_CANDIDATE
  if (!baseline || !candidate) throw new Error('Real Kimi Judge reports are required')
  await page.goto(`/evaluations/${baseline}?tab=compare&runIds=${baseline},${candidate}`)
  await expect(page.getByText('已对齐 2 / 4 个用例')).toBeVisible()
  await expect(page.getByText('baseline_only', { exact: true })).toBeVisible()
  await expect(page.getByText('candidate_only', { exact: true })).toBeVisible()
  await expect(page.getByText('缺失', { exact: true }).first()).toBeVisible()
  const traces = page.getByRole('link', { name: '评测 Trace' })
  await expect(traces).toHaveCount(6)
  await page.screenshot({ path: testInfo.outputPath('real-v1-v2-comparison.png'), fullPage: true })
  await traces.first().click()
  await expect(page).toHaveURL(/\/executions\/[a-f0-9-]+$/)
  await page.getByRole('tab', { name: '高级瀑布', exact: true }).click()
  await expect(page.getByRole('treegrid', { name: 'Trace 层级瀑布' })).toBeVisible()
  await page.screenshot({ path: testInfo.outputPath('real-judge-trace.png'), fullPage: true })
})

test('Judge profile saves the actual authorized Kimi model', async ({ page }, testInfo) => {
  await login(page)
  const alias = process.env.AGENTX_E2E_JUDGE_MODEL_ALIAS
  if (!alias) throw new Error('Real Kimi model alias is required')
  await page.goto('/evaluations?tab=profiles')
  await page.getByRole('tab', { name: '评测方案', exact: true }).click()
  await page.getByRole('button', { name: '新建评测方案', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '新建评测方案' })
  await dialog.getByRole('combobox', { name: '规则类型' }).click()
  await page.getByRole('option', { name: 'LLM Judge', exact: true }).click()
  await dialog.getByRole('combobox', { name: '评估模型' }).click()
  await page.getByRole('option', { name: alias, exact: true }).click()
  await dialog.getByRole('textbox', { name: '名称', exact: true }).fill(`Real Kimi UI Judge ${Date.now()}`)
  await dialog.getByRole('textbox', { name: '规则名称', exact: true }).fill('Kimi Judge')
  await dialog.getByPlaceholder('Evaluate whether the actual output matches… {{actualOutput}} {{expectedOutput}}').fill('Compare {{actualOutput}} with {{expectedOutput}} and return passed, score and reason.')
  const saved = page.waitForResponse((response) => response.url().endsWith('/evaluation-profiles') && response.request().method() === 'POST')
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  expect((await saved).status()).toBe(201)
  await expect(dialog).toHaveCount(0)
  await page.screenshot({ path: testInfo.outputPath('real-judge-profile-saved.png'), fullPage: true })
})

test('Studio run panel displays real Kimi model deltas and final execution', async ({ page }, testInfo) => {
  const token = await login(page)
  const appId = process.env.AGENTX_E2E_LIVE_APP_ID
  if (!appId) throw new Error('Real Kimi application is required')
  const app = await page.request.get(`/api/v1/applications/${appId}`, { headers: { Authorization: `Bearer ${token}` } })
  expect(app.ok()).toBe(true)
  const workflowId = ((await app.json()) as { workflowId: string }).workflowId
  await page.goto(`/workflows/${workflowId}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  const accepted = page.waitForResponse((response) => response.url().endsWith('/debug-executions') && response.request().method() === 'POST')
  await page.locator('header').getByRole('button', { name: '运行', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: /运行工作流|调试输入/ })
  await expect(dialog).toBeVisible()
  await dialog.getByLabel('Message', { exact: true }).fill('请以画布水星开头,分10条说明如何测试工作流,控制在400字以内。')
  await dialog.getByRole('button', { name: /^(开始运行|运行)$/ }).click()
  expect((await accepted).status()).toBe(202)
  await page.getByTestId('runtime-rail').getByRole('tab', { name: '事件', exact: true }).click()
  await expect(page.getByTestId('model-stream-tail')).toBeVisible({ timeout: 120_000 })
  await expect(page.getByTestId('model-stream-tail')).toContainText('画布水星', { timeout: 120_000 })
  await page.screenshot({ path: testInfo.outputPath('studio-real-model-deltas.png'), fullPage: true })
  const executionId = ((await (await accepted).json()) as { executionId: string }).executionId
  let output: { status: string; output: { answer?: string } } | undefined
  await expect.poll(async () => {
    const result = await page.request.get(`/api/v1/executions/${executionId}`, { headers: { Authorization: `Bearer ${token}` } })
    expect(result.ok()).toBe(true)
    output = await result.json()
    return output?.status
  }, { timeout: 180_000 }).toBe('succeeded')
  expect(output?.output.answer).toContain('画布水星')
  await expect(page.getByTestId('model-stream-tail').locator(':scope > p')).toHaveText(output!.output.answer!, { timeout: 30_000 })
  await writeFile(testInfo.outputPath('studio-stream-results.json'), JSON.stringify({ workflowId, executionId, realModelDeltaVisible: true, deltasMatchFinalOutput: true }, null, 2))
})

test('Insights error links retain the failed workflow and error-code filters', async ({ page }, testInfo) => {
  const aggregateErrors: number[] = []
  page.on('response', (response) => {
    if (response.url().endsWith('/insights/aggregates') && response.status() >= 400) aggregateErrors.push(response.status())
  })
  const token = await login(page)
  const name = `P7 real Agent ${process.env.AGENTX_E2E_RUN_ID}`
  const response = await page.request.get('/api/v1/workflows', { headers: { Authorization: `Bearer ${token}` }, params: { search: name, pageSize: 100 } })
  expect(response.ok()).toBe(true)
  const workflow = ((await response.json()) as { items: Array<{ id: string; name: string }> }).items.find((item) => item.name === name)
  if (!workflow) throw new Error('Controlled Agent failure workflow is required')
  await page.goto('/insights')
  await page.getByRole('combobox', { name: '工作流筛选', exact: true }).click()
  await page.getByRole('option', { name, exact: true }).click()
  const link = page.getByRole('link', { name: /^AGENT_SESSION_REQUIRED \(/ })
  await expect(link).toBeVisible({ timeout: 120_000 })
  await expect(page.getByText('正在加载', { exact: true })).toHaveCount(0)
  expect(aggregateErrors).toEqual([])
  await page.screenshot({ path: testInfo.outputPath('insights-controlled-error.png'), fullPage: true })
  await link.click()
  await expect(page).toHaveURL(/\/executions\?/)
  const query = new URL(page.url()).searchParams
  expect(query.get('errorCodes')).toBe('AGENT_SESSION_REQUIRED')
  expect(query.get('workflowIds')).toBe(workflow.id)
  await expect(page.getByRole('row').filter({ hasText: name }).first()).toBeVisible()
  await page.screenshot({ path: testInfo.outputPath('error-filtered-executions.png'), fullPage: true })
})
