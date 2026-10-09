import { expect, type Page, test } from '@playwright/test'

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill('agentx-e2e-admin-password')
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
}

test('compares real reports with absent cases and judge Trace links', async ({ page }, testInfo) => {
  const baseline = process.env.AGENTX_E2E_COMPARE_BASELINE
  const candidate = process.env.AGENTX_E2E_COMPARE_CANDIDATE
  if (!baseline || !candidate) throw new Error('Real comparison fixture IDs are required')
  await login(page)
  await page.goto(`/evaluations/${baseline}?tab=compare&runIds=${baseline},${candidate}`)
  await expect(page.getByText('已对齐 1 / 3 个用例')).toBeVisible()
  await expect(page.getByText('baseline_only', { exact: true })).toBeVisible()
  await expect(page.getByText('candidate_only', { exact: true })).toBeVisible()
  await expect(page.getByText('缺失', { exact: true }).first()).toBeVisible()
  const traces = page.getByRole('link', { name: '评测 Trace' })
  await expect(traces.first()).toBeVisible()
  await expect(traces).toHaveCount(4)
  await expect(traces.first()).toHaveAttribute('href', /\/executions\/[a-f0-9-]+$/)
  await page.screenshot({ path: testInfo.outputPath('comparison.png'), fullPage: true })
  const href = await traces.first().getAttribute('href')
  const loaded = page.waitForResponse((response) => response.url().endsWith(`/api/v1${href}`) && response.request().method() === 'GET')
  await traces.first().click()
  expect((await loaded).status()).toBe(200)
  await expect(page).toHaveURL(new RegExp(`${href}$`))
  await expect(page.getByRole('heading', { level: 1 })).toContainText('Compare')
  await page.screenshot({ path: testInfo.outputPath('judge-trace.png'), fullPage: true })
})

test('selects a real judge model in the profile form', async ({ page }, testInfo) => {
  const alias = process.env.AGENTX_E2E_JUDGE_MODEL_ALIAS
  if (!alias) throw new Error('Real judge model alias is required')
  await login(page)
  await page.goto('/evaluations?tab=profiles')
  await page.getByRole('tab', { name: '评测方案', exact: true }).click()
  await page.getByRole('button', { name: '新建评测方案', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '新建评测方案' })
  await dialog.getByRole('combobox', { name: '规则类型' }).click()
  await page.getByRole('option', { name: 'LLM Judge', exact: true }).click()
  await dialog.getByRole('combobox', { name: '评估模型' }).click()
  await page.getByRole('option', { name: alias, exact: true }).click()
  await expect(dialog.getByRole('link', { name: '查看模型授权' })).toBeVisible()
  const prompt = 'Evaluate {{actualOutput}} against {{expectedOutput}}'
  await dialog.getByPlaceholder('Evaluate whether the actual output matches… {{actualOutput}} {{expectedOutput}}').fill(prompt)
  await dialog.getByRole('textbox', { name: '名称', exact: true }).fill(`Browser Judge ${Date.now()}`)
  await dialog.getByRole('textbox', { name: '规则名称', exact: true }).fill('Judge')
  await page.screenshot({ path: testInfo.outputPath('judge-profile.png'), fullPage: true })
  const saved = page.waitForResponse((response) => response.url().endsWith('/api/v1/evaluation-profiles') && response.request().method() === 'POST')
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  const response = await saved
  expect(response.status()).toBe(201)
  const profile = await response.json()
  expect(profile.rules[0].evaluatorType).toBe('llm_judge')
  expect(profile.rules[0].configuration.prompt).toBe(prompt)
  expect(profile.rules[0].configuration.modelId).toMatch(/^[a-f0-9-]{36}$/)
  await expect(dialog).toBeHidden()
})
