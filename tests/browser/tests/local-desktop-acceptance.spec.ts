import { expect, type Locator, type Page, test } from '@playwright/test'
import { writeFile } from 'node:fs/promises'
import { fillMonaco, showEdgeToolbar } from './workflow-studio-editor-helpers'

const password = 'agentx-e2e-admin-password'

async function login(page: Page, username = 'admin', userPassword = password) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill(username)
  await page.getByLabel('密码').fill(userPassword)
  const result = page.waitForResponse((response) => response.url().endsWith('/api/v1/auth/login') && response.request().method() === 'POST')
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await expect(page).toHaveURL(/\/$/)
  return ((await (await result).json()) as { accessToken: string }).accessToken
}

async function api<T>(page: Page, token: string, path: string, method = 'GET', data?: unknown): Promise<T> {
  const response = await page.request.fetch(`/api/v1${path}`, { method, data, headers: { Authorization: `Bearer ${token}` } })
  if (!response.ok()) throw new Error(`${path}: ${response.status()} ${await response.text()}`)
  return response.json() as Promise<T>
}

test('desktop pages load in both languages and themes without application errors', async ({ page }, testInfo) => {
  const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  await login(page)
  for (const path of ['workflows', 'applications', 'playground', 'knowledge', 'memory', 'evaluations', 'insights', 'runtime']) {
    await page.goto(`/${path}`)
    await expect(page.getByRole('main')).toBeVisible()
    await expect(page.getByRole('heading', { level: 1 }).first()).toBeVisible()
    await expect(page.getByText('Unexpected Application Error!', { exact: false })).toHaveCount(0)
    await page.screenshot({ path: testInfo.outputPath(`${path}-zh-light.png`), fullPage: true })
  }
  await page.goto('/knowledge')
  for (const language of ['English', '简体中文']) {
    await page.getByRole('button', { name: /^(语言|Language)$/ }).click()
    await page.getByRole('menuitemradio', { name: language, exact: true }).click()
    const english = language === 'English'
    await expect(page.getByRole('link', { name: english ? 'Knowledge' : '知识库', exact: true })).toBeVisible()
    for (const dark of [true, false]) {
      await page.getByRole('button', { name: /^(主题|Theme)$/ }).click()
      await page.getByRole('menuitemradio', { name: english ? dark ? 'Dark' : 'Light' : dark ? '深色' : '浅色', exact: true }).click()
      await expect(page.locator('html')).toHaveClass(dark ? /dark/ : /^(?!.*\bdark\b).*$/)
      await page.screenshot({ path: testInfo.outputPath(`knowledge-${english ? 'en' : 'zh'}-${dark ? 'dark' : 'light'}.png`), fullPage: true })
    }
  }
  await page.goto('/page-that-does-not-exist')
  await expect(page.getByRole('heading', { name: '页面不存在' })).toBeVisible()
  await page.screenshot({ path: testInfo.outputPath('not-found.png'), fullPage: true })
  expect(errors).toEqual([])
})

test('department users cannot read another department knowledge resource or manage accounts', async ({ browser, page }, testInfo) => {
  const token = await login(page)
  const me = await api<{ departmentId: string }>(page, token, '/auth/me')
  const stamp = `${Date.now()}`
  const owner = await api<{ id: string }>(page, token, '/departments', 'POST', { parentId: me.departmentId, name: `P7 owner ${stamp}` })
  const outsider = await api<{ id: string }>(page, token, '/departments', 'POST', { parentId: me.departmentId, name: `P7 outsider ${stamp}` })
  const role = await api<{ id: string }>(page, token, '/roles', 'POST', {
    code: `p7_reader_${stamp}`, name: `P7 department reader ${stamp}`, dataScope: 'department_tree', permissions: ['knowledge:view'],
  })
  const username = `p7-reader-${stamp}`
  await api(page, token, '/users', 'POST', { username, displayName: username, departmentId: outsider.id, roleId: role.id })
  const credential = await api<{ id: string }>(page, token, '/credentials', 'POST', {
    name: `P7 scope credential ${stamp}`, credentialType: 'bearer', secret: `scope-test-${stamp}`, ownerDepartmentId: owner.id,
  })
  const connection = await api<{ id: string }>(page, token, '/knowledge/connections', 'POST', {
    name: `P7 scope connection ${stamp}`, provider: 'lightrag', endpoint: 'http://lightrag:9621', healthPath: '/health',
    credentialId: credential.id, ownerDepartmentId: owner.id, configuration: {},
  })
  const resource = await api<{ id: string }>(page, token, '/knowledge/resources', 'POST', {
    name: `P7 scoped resource ${stamp}`, connectionId: connection.id, externalResourceId: `p7_scope_${stamp}`, ownerDepartmentId: owner.id,
  })
  const firstLogin = await page.request.post('/api/v1/auth/login', { data: { username, password: '123456' } })
  expect(firstLogin.status()).toBe(200)
  const changed = await page.request.post('/api/v1/auth/change-password', {
    data: { token: ((await firstLogin.json()) as { changePasswordToken: string }).changePasswordToken, password: `P7-reader-password-${stamp}` },
  })
  expect(changed.status()).toBe(200)
  const context = await browser.newContext({ baseURL: new URL(page.url()).origin, viewport: { width: 1440, height: 900 }, locale: 'zh-CN' })
  try {
    const reader = await context.newPage()
    const readerToken = await login(reader, username, `P7-reader-password-${stamp}`)
    const headers = { Authorization: `Bearer ${readerToken}` }
    const resources = await reader.request.get('/api/v1/knowledge/resources?pageSize=100', { headers })
    expect(resources.status()).toBe(200)
    expect(((await resources.json()) as { items: Array<{ id: string }> }).items.some((item) => item.id === resource.id)).toBe(false)
    const denied = await reader.request.get(`/api/v1/knowledge/resources/${resource.id}/documents`, { headers })
    expect([403, 404]).toContain(denied.status())
    const manage = await reader.request.post('/api/v1/users', { headers, data: { username: `forbidden-${stamp}`, displayName: 'Forbidden account', departmentId: outsider.id, roleId: role.id } })
    expect(manage.status()).toBe(403)
    await reader.goto('/knowledge')
    await expect(reader.getByText(`P7 scoped resource ${stamp}`, { exact: true })).toHaveCount(0)
    await expect(reader.getByRole('button', { name: /新建知识|新增知识/ })).toHaveCount(0)
    await reader.screenshot({ path: testInfo.outputPath('department-reader.png'), fullPage: true })
    await writeFile(testInfo.outputPath('scope-results.json'), JSON.stringify({ resourceId: resource.id, foreignReadStatus: denied.status(), accountWriteStatus: manage.status() }, null, 2))
  } finally {
    await context.close()
  }
})

async function connect(page: Page, source: Locator, target: Locator) {
  const edges = page.locator('.react-flow__edge')
  const count = await edges.count()
  const from = source.locator('.react-flow__handle.source[data-handleid="main"]')
  const to = target.locator('.react-flow__handle.target[data-handleid="main"]')
  for (let attempt = 0; attempt < 3 && await edges.count() === count; attempt += 1) {
    const start = await from.boundingBox()
    const end = await to.boundingBox()
    if (!start || !end) throw new Error('Workflow ports are not visible')
    await page.mouse.move(start.x + start.width / 2, start.y + start.height / 2)
    await page.mouse.down()
    await page.mouse.move(end.x + end.width / 2, end.y + end.height / 2, { steps: 20 })
    await page.waitForTimeout(125)
    await page.mouse.up()
    await expect.poll(async () => edges.count(), { timeout: 2000 }).toBeGreaterThan(count).catch(() => undefined)
  }
  await expect(edges).toHaveCount(count + 1)
}

test('Python Code is configured through Studio and executes in a real OpenSandbox', async ({ page }, testInfo) => {
  const token = await login(page)
  const me = await api<{ departmentId: string }>(page, token, '/auth/me')
  const sandboxName = `P7 Python Sandbox ${Date.now()}`
  await api(page, token, '/sandbox-profiles', 'POST', {
    name: sandboxName, description: 'P7 real Python UI acceptance', ownerDepartmentId: me.departmentId,
    runner: 'python', imageDigest: process.env.AGENTX_E2E_SANDBOX_IMAGE,
    cpuMillis: 500, memoryBytes: 536870912, pidsLimit: 256, diskBytes: 1073741824,
    timeoutSeconds: 120, outputLimitBytes: 1048576, networkPolicy: { defaultAction: 'deny', egressMode: 'none' },
  })
  const name = `P7 Python Workflow ${Date.now()}`
  await page.goto('/workflows')
  await page.getByRole('button', { name: '新建工作流' }).click()
  const creation = page.getByRole('dialog', { name: '新建工作流' })
  await creation.getByLabel('工作流名称').fill(name)
  await creation.getByLabel('描述').fill('Real Python sandbox without a cloud model')
  await creation.getByRole('button', { name: '保存', exact: true }).click()
  await expect(page).toHaveURL(/\/workflows\/[a-f0-9-]+$/)
  const workflowId = page.url().split('/').at(-1)!
  await page.goto('/resource-grants')
  await page.getByRole('tab', { name: '沙箱配置', exact: true }).click()
  await page.getByRole('searchbox', { name: '搜索资源名称、类型或连接信息' }).fill(sandboxName)
  await page.getByRole('row').filter({ hasText: sandboxName }).getByRole('button', { name: '管理授权' }).click()
  const grant = page.getByRole('dialog').filter({ hasText: sandboxName })
  await grant.getByRole('combobox').nth(1).click()
  await page.getByRole('option', { name, exact: true }).click()
  await grant.getByRole('button', { name: '添加授权', exact: true }).click()
  await expect(grant.getByRole('button', { name: '撤销授权' })).toHaveCount(1)
  await grant.getByRole('button', { name: '取消', exact: true }).click()
  await page.goto(`/workflows/${workflowId}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  const initial = page.locator('.react-flow__edge[data-testid="rf__edge-start-exit"]')
  const toolbar = page.locator('.studio-edge-toolbar[data-edge-id="start-exit"]')
  await showEdgeToolbar(initial, toolbar)
  await toolbar.getByRole('button', { name: /删除连线|Delete connection/ }).dispatchEvent('click')
  await page.getByRole('textbox', { name: '搜索节点' }).fill('code')
  await page.getByTestId('palette-action-code').click()
  const code = page.locator('.react-flow__node-manifest').filter({ hasText: /代码|Code/ }).first()
  await code.dispatchEvent('click')
  const details = page.getByTestId('node-details-view')
  await expect(details).toBeVisible()
  const runner = details.getByTestId('parameter-runner')
  await runner.getByRole('combobox').click()
  await page.getByRole('option', { name: /Python/ }).click()
  const inputs = details.getByTestId('parameter-inputs')
  for (const [key, value] of [['question', '水星蓝桥'], ['count', '2']]) {
    await inputs.getByRole('button', { name: /添加字段|Add field/ }).click()
    const field = inputs.getByRole('textbox', { name: /键|Key/ }).last()
    await field.fill(key)
    await field.blur()
    await inputs.getByRole('textbox', { name: 'Value' }).last().fill(value)
  }
  const pythonSource = 'def main(**inputs):\n    print("p7-python-ok")\n    return {"answer": inputs["question"], "count": int(inputs["count"])}'
  await fillMonaco(page, details.getByTestId('parameter-source'), pythonSource, true)
  await fillMonaco(page, details.getByTestId('code-output-example'), '{ answer: "", count: 0 }')
  await details.getByTestId('resource-selector-sandbox_profile').getByRole('combobox').click()
  await page.getByRole('option', { name: new RegExp(sandboxName) }).click()
  await details.getByRole('button', { name: /^(关闭|Close)$/ }).first().click()
  await page.getByRole('button', { name: /^(适应画布|Fit view)$/ }).click()
  await connect(page, page.getByTestId('workflow-start'), code)
  await connect(page, code, page.getByTestId('exit-node-exit'))
  const saved = page.waitForResponse((response) => response.url().endsWith(`/api/v1/workflows/${workflowId}/draft`) && response.request().method() === 'PUT')
  await page.getByRole('button', { name: '保存', exact: true }).click()
  const saveResponse = await saved
  expect(saveResponse.ok()).toBe(true)
  const definition = saveResponse.request().postDataJSON().definition as { nodes: Array<{ type: string; parameters: { source?: string } }> }
  expect(definition.nodes.find((node) => node.type === 'code')?.parameters.source).toBe(pythonSource)
  const executionResponse = page.waitForResponse((response) => response.url().includes('/debug-executions') && response.request().method() === 'POST')
  await page.locator('header').getByRole('button', { name: '运行', exact: true }).click()
  const parameters = page.getByRole('dialog', { name: /运行工作流|调试输入|Run workflow|Debug input/ })
  if (await parameters.waitFor({ state: 'visible', timeout: 3000 }).then(() => true).catch(() => false)) {
    const question = parameters.getByLabel(/Question|问题/)
    if (await question.isVisible().catch(() => false)) await question.fill('Python sandbox test')
    await parameters.getByRole('button', { name: /^(开始运行|运行|Run workflow|Run)$/ }).click()
  }
  const accepted = await executionResponse
  expect(accepted.status()).toBe(202)
  const executionId = ((await accepted.json()) as { executionId: string }).executionId
  await expect.poll(async () => {
    const execution = await api<{ status: string; error?: unknown }>(page, token, `/executions/${executionId}`)
    if (execution.status === 'failed') throw new Error(JSON.stringify(execution))
    return execution.status
  }, { timeout: 180_000 }).toBe('succeeded')
  const nodes = await api<{ items: Array<{ nodeType: string; nodeName: string; output?: { main?: Array<{ json: Record<string, unknown> }> } }> }>(page, token, `/executions/${executionId}/nodes`)
  const result = nodes.items.find((node) => node.nodeType === 'code' || node.output?.main?.[0]?.json.exitCode === 0)?.output?.main?.[0]?.json
  expect(result).toMatchObject({ exitCode: 0, structuredOutput: { answer: '水星蓝桥', count: 2 } })
  expect(result?.stdout).toContain('p7-python-ok')
  await expect.poll(async () => (await api<{ sandboxes: Array<{ status: string }> }>(page, token, `/executions/${executionId}/runtime-details`)).sandboxes.map((item) => item.status), { timeout: 30_000 }).toEqual(['terminated'])
  await page.goto(`/executions/${executionId}`)
  await page.getByRole('tab', { name: '高级瀑布', exact: true }).click()
  await expect(page.getByRole('treegrid', { name: 'Trace 层级瀑布' })).toBeVisible()
  await page.screenshot({ path: testInfo.outputPath('python-sandbox-execution.png'), fullPage: true })
  await writeFile(testInfo.outputPath('python-sandbox-results.json'), JSON.stringify({ workflowId, executionId, output: result, sandboxTerminated: true }, null, 2))
})
