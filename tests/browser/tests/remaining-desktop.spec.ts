import { expect, type Page, test } from '@playwright/test'
import { writeFile } from 'node:fs/promises'

function required(name: string): string {
  const value = process.env[name]
  if (!value) throw new Error(`Missing ${name}`)
  return value
}

async function login(page: Page, username = 'admin', password = 'agentx-e2e-admin-password') {
  await page.goto('/login')
  await page.getByLabel('用户名').fill(username)
  await page.getByLabel('密码').fill(password)
  const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/auth/login') && value.request().method() === 'POST')
  await page.getByRole('button', { name: '登录', exact: true }).click()
  const token = (await (await response).json()).accessToken as string
  await expect(page).toHaveURL(/\/$/)
  return token
}

const lists = [
  ['workflows', 'workflows'], ['applications', 'applications'], ['models', 'models/aliases'],
  ['credentials', 'credentials'], ['mcp', 'mcp/servers'], ['skills', 'skills'],
  ['knowledge', 'knowledge/resources'], ['memory', 'memory/namespaces'], ['evaluations', 'evaluations'],
] as const

for (const [path, api] of lists) {
  test(`${path} shows loading, failure, recovery and an honest empty search`, async ({ page }, testInfo) => {
    const errors: string[] = []
    page.on('pageerror', (error) => errors.push(error.message))
    await login(page)
    let release!: () => void
    const gate = new Promise<void>((resolve) => { release = resolve })
    const matcher = new RegExp(`/api/v1/${api}(?:\\?|$)`)
    await page.route(matcher, async (route) => {
      if (route.request().method() !== 'GET') return route.continue()
      await gate
      await route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ code: 'P7_TEST_UNAVAILABLE', message: 'P7 暂时不可用', requestId: 'p7-desktop-fault' }) })
    })
    try {
      await page.goto(`/${path}`)
      await expect(page.getByRole('status').filter({ hasText: '正在加载' })).toBeVisible()
    } finally { release() }
    await expect(page.getByRole('alert').filter({ hasText: '数据加载失败' })).toBeVisible()
    await expect(page.getByRole('alert')).toContainText('请求暂时无法完成')
    await expect(page.getByRole('alert')).toContainText('p7-desktop-fault')
    await expect(page.getByText('没有找到匹配结果', { exact: true })).toHaveCount(0)
    await page.screenshot({ path: testInfo.outputPath(`${path}-failure.png`), fullPage: true })
    await page.unroute(matcher)
    await page.reload()
    await expect(page.getByRole('alert').filter({ hasText: '数据加载失败' })).toHaveCount(0)
    await page.getByRole('main').getByRole('searchbox').fill('p7-deliberately-no-such-resource-915840')
    await expect(page.getByText('没有找到匹配结果', { exact: true })).toBeVisible()
    await page.screenshot({ path: testInfo.outputPath(`${path}-empty.png`), fullPage: true })
    expect(errors).toEqual([])
    await writeFile(testInfo.outputPath(`${path}-results.json`), JSON.stringify({ loading: true, injected503: true, recoveredRealApi: true, emptySearch: true, pageErrors: errors }, null, 2))
  })
}

test('Runtime failure never displays healthy zeroes and recovers', async ({ page }, testInfo) => {
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  await login(page)
  let release!: () => void
  const gate = new Promise<void>((resolve) => { release = resolve })
  const matcher = /\/api\/v1\/runtime\/status(?:\?|$)/
  await page.route(matcher, async (route) => {
    await gate
    await route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ code: 'P7_TEST_UNAVAILABLE', message: 'P7 Runtime 暂时不可用' }) })
  })
  try {
    await page.goto('/runtime')
    await expect(page.getByRole('status').filter({ hasText: '正在加载' })).toBeVisible()
  } finally { release() }
  await expect(page.getByRole('alert').filter({ hasText: '数据加载失败' })).toBeVisible()
  await expect(page.getByRole('main').getByText('0', { exact: true })).toHaveCount(0)
  await page.screenshot({ path: testInfo.outputPath('runtime-unavailable.png'), fullPage: true })
  await page.unroute(matcher)
  const restored = page.waitForResponse((value) => matcher.test(value.url()) && value.status() === 200)
  await page.reload()
  const restoredStatus = await (await restored).json() as Record<string, number | null>
  await expect(page.getByRole('alert').filter({ hasText: '数据加载失败' })).toHaveCount(0)
  await expect(page.getByRole('main').getByRole('heading', { name: '运行组件', exact: true })).toBeVisible()
  await expect(page.getByRole('main').getByText('运行服务', { exact: true })).toBeVisible()
  await expect(page.getByRole('main').getByText('画布插件（Node.js）', { exact: true })).toBeVisible()
  const keys = ['running', 'waiting', 'failedToday', 'activeSandboxes']
  const metrics = page.getByRole('main').locator('section strong')
  await expect(metrics).toHaveText(keys.map((key) => restoredStatus[key] === null ? '—' : String(restoredStatus[key])))
  await page.screenshot({ path: testInfo.outputPath('runtime-recovered.png'), fullPage: true })
  await page.route(matcher, (route) => route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ...restoredStatus, running: 0, waiting: 0, failedToday: 0, activeSandboxes: 0 }) }))
  await page.reload()
  await expect(metrics).toHaveText(['0', '0', '0', '0'])
  await page.screenshot({ path: testInfo.outputPath('runtime-real-zeroes.png'), fullPage: true })
  await page.unroute(matcher)
  expect(errors).toEqual([])
})

test('five real evaluations display absent cases and follow changed baseline order', async ({ page }, testInfo) => {
  const ids = JSON.parse(required('AGENTX_E2E_FIVE_COMPARE_IDS')) as string[]
  const names = JSON.parse(required('AGENTX_E2E_FIVE_COMPARE_NAMES')) as string[]
  await login(page)
  for (const order of [ids, [...ids].reverse()]) {
    await page.goto(`/evaluations/${order[0]}?${new URLSearchParams({ tab: 'compare', runIds: order.join(',') })}`)
    await expect(page.getByText('已对齐 1 / 5 个用例', { exact: true })).toBeVisible()
    const headers = page.getByRole('table').first().getByRole('columnheader')
    await expect(headers).toHaveCount(6)
    await expect(headers.nth(1)).toHaveText(names[ids.indexOf(order[0])])
    const missing = page.getByRole('row').filter({ has: page.getByRole('cell', { name: 'baseline_only', exact: true }) })
    await expect(missing.getByRole('cell', { name: '缺失', exact: true })).toHaveCount(4)
    await expect(page.getByRole('link', { name: '评测 Trace', exact: true }).first()).toBeVisible()
    await page.screenshot({ path: testInfo.outputPath(`comparison-baseline-${ids.indexOf(order[0])}.png`), fullPage: true })
  }
  await page.getByRole('button', { name: '选择评估运行', exact: true }).click()
  await page.getByRole('option', { name: names[4], exact: true }).click()
  await expect(page).toHaveURL(new RegExp(`runIds=${ids[3]}`))
  await expect(page.getByText('已对齐 1 / 4 个用例', { exact: true })).toBeVisible()
  await page.keyboard.press('Escape')
  await page.getByRole('link', { name: '评测 Trace', exact: true }).first().click()
  await expect(page).toHaveURL(/\/executions\//)
  await expect(page.getByRole('main').getByRole('heading', { level: 1 }).first()).toBeVisible()
  await expect(page.getByText('Unexpected Application Error!', { exact: false })).toHaveCount(0)
})

test('permission denial and revocation refresh the existing browser session', async ({ browser }, testInfo) => {
  const options = { baseURL: required('AGENTX_E2E_BASE_URL'), viewport: { width: 1440, height: 900 }, locale: 'zh-CN' }
  const adminContext = await browser.newContext(options)
  const userContext = await browser.newContext(options)
  try {
    const admin = await adminContext.newPage()
    const user = await userContext.newPage()
    const token = await login(admin)
    await login(user, required('AGENTX_E2E_PERMISSION_USER'), required('AGENTX_E2E_PERMISSION_PASSWORD'))
    await user.goto('/credentials')
    await expect(user).toHaveURL(/\/403$/)
    await user.goto('/workflows')
    await expect(user.getByRole('heading', { level: 1 }).first()).toBeVisible()
    const role = JSON.parse(required('AGENTX_E2E_PERMISSION_ROLE')) as { id: string; name: string; description: string | null; version: number }
    const revoked = await admin.request.patch(`/api/v1/roles/${role.id}`, {
      headers: { Authorization: `Bearer ${token}` },
      data: { name: role.name, description: role.description, dataScope: 'company', permissions: [], version: role.version },
    })
    expect(revoked.status()).toBe(200)
    await user.reload()
    await expect(user).toHaveURL(/\/(403|login)$/)
    await expect(user.getByText('Unexpected Application Error!', { exact: false })).toHaveCount(0)
    await user.screenshot({ path: testInfo.outputPath('permission-revoked.png'), fullPage: true })
  } finally {
    await adminContext.close()
    await userContext.close()
  }
})

test('desktop states remain usable in English and both themes', async ({ page }, testInfo) => {
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  await login(page)
  await page.getByRole('button', { name: '语言', exact: true }).click()
  await page.getByRole('menuitemradio', { name: 'English', exact: true }).click()
  for (const dark of [true, false]) {
    await page.getByRole('button', { name: 'Theme', exact: true }).click()
    await page.getByRole('menuitemradio', { name: dark ? 'Dark' : 'Light', exact: true }).click()
    for (const [path, title] of [['/knowledge', 'Knowledge'], ['/runtime', 'Runtime status'], ['/evaluations', 'Evaluation Reports']]) {
      await page.goto(path)
      await expect(page.getByRole('main').getByRole('heading', { name: title, exact: true })).toBeVisible()
      if (path === '/runtime') {
        await expect(page.getByRole('main').getByText('Runtime service', { exact: true })).toBeVisible()
        await expect(page.getByRole('main').getByText('Canvas plugins (Node.js)', { exact: true })).toBeVisible()
      }
      await expect(page.locator('html')).toHaveClass(dark ? /dark/ : /^(?!.*\bdark\b).*$/)
      await expect(page.getByText('Unexpected Application Error!', { exact: false })).toHaveCount(0)
      await page.screenshot({ path: testInfo.outputPath(`${path.replaceAll('/', '-')}-en-${dark ? 'dark' : 'light'}.png`), fullPage: true })
    }
  }
  expect(errors).toEqual([])
})
