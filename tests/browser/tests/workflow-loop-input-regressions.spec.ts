import { resizeLoop } from './workflow-studio-editor-helpers'
import { assertRoutesAvoidCards, closeDetails, waitForCanvasSettled } from './workflow-routing-helpers'
import { fixtureVersionId } from '../draft-fixtures'
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
  await page.goto(`/workflows/${page.url().split('/').at(-1)}/versions/${await fixtureVersionId(page, page.url().split('/').at(-1)!)}/editor`)
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
  await expect(loopNode.locator('.react-flow__handle.target')).toHaveAttribute('aria-label', /输入|Input/)
  await expect(loopNode.locator('.react-flow__handle.source[data-handleid="main"]')).toHaveAttribute('aria-label', /输出|Output/)
  await expect(loopNode.getByText(/^(错误|Error)$/)).toBeVisible()
  await expect(loopNode.getByRole('button', { name: /输出.*添加节点|Add node after.*Output/i })).toBeVisible()
  await expect(loopNode.getByRole('button', { name: /错误.*添加节点|Add node after.*Error/i })).toBeVisible()
  await expect(page.getByTestId(`studio-end-chip-${loopId}`).locator('.react-flow__handle.target')).toHaveCount(2)
  await page.getByTestId('node-details-view').getByRole('button', { name: /^(关闭|Close)$/ }).click()

  await page.getByRole('button', { name: /^(适应画布|Fit view)$/ }).click()
  await waitForCanvasSettled(page)
  await resizeLoop(page, frame, frame.locator('.studio-loop-resize-handle.right.bottom'))

  await page.getByRole('button', { name: /添加循环体节点|Add loop body node/ }).click()
  await addAction(page, 'set')
  const firstStep = page.locator('.react-flow__node.selected')
  const firstId = await firstStep.getAttribute('data-id')
  await firstStep.getByRole('button', { name: /输出.*添加节点|Add node after.*Output/i }).click()
  await addAction(page, 'set')
  const secondId = await page.locator('.react-flow__node.selected').getAttribute('data-id')
  await expect(page.getByTestId('workflow-canvas')).toHaveAttribute('data-canonical-edge-count', '1')
  await expect(page.locator('[data-route-id^="loop-entry:"]')).toHaveCount(1)
  await expect(page.locator('[data-route-id^="loop-exit:"]')).toHaveCount(2)
  await assertRoutesAvoidCards(page, '.react-flow__node-loop-container')

  const rows = page.getByTestId('set-value-rows')
  await rows.getByRole('button', { name: /新建|Create/ }).click()
  const row = rows.locator(':scope > div').first()
  await row.getByRole('textbox', { name: /名称|Name/ }).fill('result')
  const value = row.getByRole('textbox', { name: 'Value' })
  await value.pressSequentially('12')
  await expect(value).toHaveText('12')

  await closeDetails(page)
  await page.locator(`.react-flow__node[data-id="${firstId}"] .studio-card-head`).click()
  await page.keyboard.down('Shift')
  await page.locator(`.react-flow__node[data-id="${secondId}"] .studio-card-head`).click()
  await page.keyboard.up('Shift')
  await closeDetails(page)
  await page.getByTestId('node-creator-shell').getByRole('button', { name: '分组', exact: true }).click()
  const groupId = await page.locator('[data-testid^="studio-group-"]').first().getAttribute('data-testid')
  const group = page.getByTestId(groupId!)
  await group.getByRole('button', { name: '折叠分组' }).click()
  await assertRoutesAvoidCards(page, '.react-flow__node-loop-container')
  const beforeArrangeWidth = await loopNode.evaluate(element => parseFloat((element as HTMLElement).style.width))
  await page.getByRole('button', { name: /^(自动布局|Auto layout)$/ }).click()
  await expect.poll(() => loopNode.evaluate(element => parseFloat((element as HTMLElement).style.width))).toBeLessThan(beforeArrangeWidth)
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/ }).click()
  await waitForCanvasSettled(page)
  await assertRoutesAvoidCards(page, '.react-flow__node-loop-container')
  const compactWidth = await loopNode.evaluate(element => parseFloat((element as HTMLElement).style.width))
  await group.getByRole('button', { name: '展开分组' }).click()
  await expect.poll(() => loopNode.evaluate(element => parseFloat((element as HTMLElement).style.width))).toBeGreaterThan(compactWidth)
  await assertRoutesAvoidCards(page, '.react-flow__node-loop-container, .react-flow__node-group:has(.studio-group-expanded)')
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/ }).click()
  await waitForCanvasSettled(page)
  await loopNode.locator('.studio-card-head').click()
  await page.keyboard.down('Shift')
  await page.getByTestId('exit-node-exit').locator('.studio-card-head').click()
  await page.keyboard.up('Shift')
  await closeDetails(page)
  await page.getByTestId('node-creator-shell').getByRole('button', { name: '分组', exact: true }).click()
  const outer = page.locator(`[data-testid^="studio-group-"]:not([data-testid="${groupId}"])`).first()
  await outer.getByRole('button', { name: '折叠分组' }).click()
  await expect(group).toHaveCount(0)
  await expect(loopNode).toHaveCount(0)
  await expect(page.locator('[data-route-id]')).toHaveCount(0)
  await outer.getByRole('button', { name: '展开分组' }).click()
  await expect(group).toHaveCount(1)
  await expect(page.locator('[data-route-id]')).toHaveCount(4)
  await assertRoutesAvoidCards(page, '.react-flow__node-loop-container, .react-flow__node-group:has(.studio-group-expanded)')
  await expect(page.locator('header').getByText(/修订号 \d+ · 已保存/)).toBeVisible({ timeout: 90_000 })
  await page.reload()
  await expect(page.locator('[data-route-id]')).toHaveCount(4)
  await assertRoutesAvoidCards(page, '.react-flow__node-loop-container, .react-flow__node-group:has(.studio-group-expanded)')
  const contained = await page.locator('[data-route-id]').evaluateAll((routes, id) => routes.every(route => {
    const path = route.querySelector('path') as SVGPathElement
    const inverse = path.getScreenCTM()!.inverse()
    const frame = document.querySelector(`[data-testid="studio-loop-${id}"]`)!.getBoundingClientRect()
    const a = new DOMPoint(frame.left, frame.top).matrixTransform(inverse), b = new DOMPoint(frame.right, frame.bottom).matrixTransform(inverse)
    return (JSON.parse(route.getAttribute('data-route-points')!) as Array<{ x: number; y: number }>).every(p => p.x >= a.x - .5 && p.x <= b.x + .5 && p.y >= a.y - .5 && p.y <= b.y + .5)
  }), loopId)
  expect(contained, 'Internal routes must remain inside the loop after group expansion and refresh').toBe(true)
})
