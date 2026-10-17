import { expect, type Page, test } from '@playwright/test'

const executionId = process.env.AGENTX_E2E_DEGRADED_EXECUTION_ID ?? ''
const phase = process.env.AGENTX_E2E_TRACE_PHASE ?? 'degraded'
const password = 'agentx-e2e-admin-password'

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill(password)
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
}

test('Trace keeps authoritative node data when diagnostics are unavailable and recovers later', async ({ page }) => {
  test.skip(!executionId, 'AGENTX_E2E_DEGRADED_EXECUTION_ID is required')
  await login(page)
  await page.goto(`/executions/${executionId}`)

  const workspace = page.getByTestId('trace-workspace')
  await expect(workspace).toBeVisible({ timeout: 30_000 })
  await expect(workspace.getByTestId('trace-final-output')).toBeVisible()
  await expect(workspace.getByTestId('trace-start-boundary')).toBeVisible()
  await expect(workspace.getByTestId('trace-node-card').first()).toBeVisible()
  await expect(workspace.getByTestId('trace-end-boundary')).toBeVisible()

  const start = workspace.getByTestId('trace-start-boundary')
  await start.getByRole('button').click()
  await expect(start).toContainText(/工作流输入|Workflow input/)

  const node = workspace.getByTestId('trace-node-card').first()
  const nodeToggle = node.getByRole('button').first()
  if (await nodeToggle.getAttribute('aria-expanded') !== 'true') await nodeToggle.click()
  await node.getByRole('button', { name: '输入与解析参数', exact: true }).click()
  await node.getByRole('button', { name: '语义输出', exact: true }).click()
  await expect(node.getByText('上游 Item')).toBeVisible()
  await expect(node.getByText('语义输出')).toBeVisible()

  await workspace.getByRole('tab', { name: '高级瀑布' }).click()
  if (phase === 'degraded') {
    await expect(workspace.getByText('Trace 暂时不可用')).toBeVisible({ timeout: 30_000 })
  } else {
    const waterfall = workspace.getByRole('treegrid', { name: 'Trace 层级瀑布' })
    await expect(waterfall).toBeVisible({ timeout: 60_000 })
    await expect(waterfall.locator('[role="row"][aria-level]').first()).toBeVisible({ timeout: 60_000 })
  }
})
