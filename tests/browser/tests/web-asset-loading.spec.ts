import { expect, type Page, test } from '@playwright/test'

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill('agentx-e2e-admin-password')
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await expect(page).toHaveURL(/\/$/)
}

async function createWorkflow(page: Page, name: string) {
  await page.getByRole('link', { name: /^工作流/ }).click()
  await page.getByRole('button', { name: '新建工作流', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '新建工作流', exact: true })
  await dialog.getByLabel('工作流名称').fill(name)
  const response = page.waitForResponse((item) => item.request().method() === 'POST' && item.url().endsWith('/api/v1/workflows'))
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  expect((await response).status()).toBe(201)
}

test('serves current HTML and real assets without substituting HTML for missing JavaScript', async ({ request }) => {
  for (const path of ['/', '/index.html', '/workflows/00000000-0000-0000-0000-000000000001', '/runtime-config.js']) {
    const response = await request.get(path)
    expect(response.status()).toBe(200)
    expect(response.headers()['cache-control']).toBe('no-store')
  }
  const index = await request.get('/')
  const script = (await index.text()).match(/<script type="module"[^>]*src="([^"]+)"/)
  expect(script).not.toBeNull()
  const asset = await request.get(script![1])
  expect(asset.status()).toBe(200)
  expect(asset.headers()['content-type']).toMatch(/javascript/)
  expect(asset.headers()['cache-control']).toContain('immutable')
  const missing = await request.get('/assets/workflow-detail-page-CgJqAL-D.js')
  expect(missing.status()).toBe(404)
  expect(missing.headers()['cache-control']).toBe('no-store')
  expect(await missing.text()).not.toContain('<div id="root">')
})

test('creates a workflow and opens its details and editor', async ({ page }, testInfo) => {
  await login(page)
  const name = `Workflow details ${Date.now()}`
  await createWorkflow(page, name)
  await expect(page).toHaveURL(/\/workflows\/[a-f0-9-]+$/)
  await expect(page.getByRole('heading', { name, exact: true })).toBeVisible()
  await expect(page.getByText('Unexpected Application Error!')).toHaveCount(0)
  await page.screenshot({ path: testInfo.outputPath('workflow-created-details.png'), fullPage: true })
  await page.getByRole('link', { name: '打开画布', exact: true }).click()
  await expect(page).toHaveURL(/\/workflows\/[a-f0-9-]+\/editor$/)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
})

test('recovers one missing detail chunk without submitting workflow creation twice', async ({ page }, testInfo) => {
  await login(page)
  let blocked = 0
  let creations = 0
  page.on('request', (request) => {
    if (request.method() === 'POST' && request.url().endsWith('/api/v1/workflows')) creations += 1
  })
  await page.route('**/assets/workflow-detail-page-*.js', async (route) => {
    if (blocked === 0) {
      blocked += 1
      await route.fulfill({ status: 404, contentType: 'text/plain', body: 'Chunk removed by deployment' })
    } else {
      await route.continue()
    }
  })
  const name = `Recovered workflow ${Date.now()}`
  await createWorkflow(page, name)
  await expect(page.getByRole('heading', { name, exact: true })).toBeVisible()
  expect(blocked).toBe(1)
  expect(creations).toBe(1)
  await expect(page).toHaveURL(/\/workflows\/[a-f0-9-]+$/)
  await page.screenshot({ path: testInfo.outputPath('workflow-chunk-recovered.png'), fullPage: true })
})

test('shows a usable error page after persistent chunk failures and permits an explicit retry', async ({ page }, testInfo) => {
  await login(page)
  let failures = 0
  let documents = 0
  page.on('request', (request) => { if (request.isNavigationRequest() && request.resourceType() === 'document') documents += 1 })
  await page.route('**/assets/workflow-detail-page-*.js', async (route) => {
    failures += 1
    await route.fulfill({ status: 404, contentType: 'text/plain', body: 'Persistent asset failure' })
  })
  const name = `Retry workflow ${Date.now()}`
  await createWorkflow(page, name)
  await expect(page.getByRole('heading', { name: '页面暂时无法打开' })).toBeVisible()
  expect(failures).toBe(2)
  expect(documents).toBe(1)
  await page.screenshot({ path: testInfo.outputPath('route-error-page.png'), fullPage: true })
  await page.unroute('**/assets/workflow-detail-page-*.js')
  await page.getByRole('button', { name: '刷新页面', exact: true }).click()
  await expect(page.getByRole('heading', { name, exact: true })).toBeVisible()
})
