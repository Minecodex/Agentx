import { fixtureDraftId } from '../draft-fixtures'
import { expect, type Page, test } from '@playwright/test'

const password = 'agentx-e2e-admin-password'

async function login(page: Page) {
  const status = await page.request.get('/api/v1/bootstrap/status')
  if (((await status.json()) as { required: boolean }).required) {
    await page.goto('/setup')
    await page.getByLabel('公司名称').fill('Agentx E2E')
    await page.getByLabel('用户名').fill('admin')
    await page.getByLabel('管理员姓名').fill('E2E Admin')
    await page.getByLabel('密码').fill(password)
    await page.getByRole('button', { name: '初始化并进入工作台' }).click()
  } else {
    await page.goto('/login')
    await page.getByLabel('用户名').fill('admin')
    await page.getByLabel('密码').fill(password)
    await page.getByRole('button', { name: '登录' }).click()
  }
  await expect(page).toHaveURL(/\/$/)
}

async function createWorkflow(page: Page) {
  await page.getByRole('link', { name: '工作流', exact: true }).click()
  await page.getByRole('button', { name: '新建工作流' }).click()
  const dialog = page.getByRole('dialog', { name: '新建工作流' })
  await dialog.getByLabel('工作流名称').fill(`Loop interaction ${Date.now()}`)
  await dialog.getByRole('button', { name: '保存' }).click()
  await expect(page).toHaveURL(/\/workflows\/[0-9a-f-]+$/)
  await page.goto(`/workflows/${page.url().split('/').at(-1)}/drafts/${await fixtureDraftId(page, page.url().split('/').at(-1)!)}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
}

async function addAction(page: Page, nodeType: string) {
  const creator = page.getByTestId('node-creator')
  await expect(creator).toBeVisible()
  await creator.getByRole('textbox', { name: /搜索节点|Search nodes/ }).fill(nodeType)
  await page.getByTestId(`palette-action-${nodeType}`).click()
}

test('Workflow Loop supports real resize, body chaining, boundary ports, and stable sequential value input', async ({ page }) => {
  await login(page)
  await createWorkflow(page)
  const edges = page.locator('.react-flow__edge')
  if (await edges.count() === 1) {
    await edges.first().click({ force: true })
    await page.keyboard.press('Delete')
  }

  await addAction(page, 'loop_over_items')
  const loopNode = page.locator('.react-flow__node-loop-container').first()
  const loopId = await loopNode.getAttribute('data-id')
  const frame = page.getByTestId(`studio-loop-${loopId}`)
  await expect(loopNode.getByText(/^(输入|Input)$/)).toBeVisible()
  await expect(loopNode.getByText(/^(输出|Output)$/)).toBeVisible()
  await expect(loopNode.getByText(/^(错误|Error)$/)).toBeVisible()
  await expect(loopNode.getByRole('button', { name: /输出.*添加节点|Add node after.*Output/i })).toBeVisible()
  await expect(loopNode.getByRole('button', { name: /错误.*添加节点|Add node after.*Error/i })).toBeVisible()
  await expect(page.getByTestId(`studio-end-chip-${loopId}`).locator('.react-flow__handle.target')).toHaveCount(2)
  await page.getByTestId('node-details-view').getByRole('button', { name: /^(关闭|Close)$/ }).click()

  const before = await frame.boundingBox()
  const corner = frame.locator('.studio-loop-resize-handle.right.bottom')
  const handle = await corner.boundingBox()
  expect(before).toBeTruthy()
  expect(handle).toBeTruthy()
  expect(handle!.width).toBeGreaterThanOrEqual(14)
  await page.mouse.move(handle!.x + handle!.width / 2, handle!.y + handle!.height / 2)
  await page.mouse.down()
  await page.mouse.move(handle!.x + handle!.width / 2 + 120, handle!.y + handle!.height / 2 + 80, { steps: 12 })
  await page.mouse.up()
  await expect.poll(async () => (await frame.boundingBox())?.width ?? 0).toBeGreaterThan(before!.width + 80)

  await page.getByRole('button', { name: /添加循环体节点|Add loop body node/ }).click()
  await addAction(page, 'set')
  const firstStep = page.locator('.react-flow__node.selected')
  await firstStep.getByRole('button', { name: /输出.*添加节点|Add node after.*Output/i }).click()
  await addAction(page, 'set')
  await expect(edges).toHaveCount(1)
  await expect(frame.locator('[data-loop-boundary-edge="entry"]')).toHaveCount(1)
  await expect(frame.locator('[data-loop-boundary-edge="main"]')).toHaveCount(1)
  await expect(frame.locator('[data-loop-boundary-edge="error"]')).toHaveCount(1)

  const rows = page.getByTestId('set-value-rows')
  await rows.getByRole('button', { name: /新建|Create/ }).click()
  const row = rows.locator(':scope > div').first()
  await row.getByRole('textbox', { name: /名称|Name/ }).fill('result')
  const value = row.getByRole('textbox', { name: 'Value' })
  await value.pressSequentially('12')
  await expect(value).toHaveText('12')
})
