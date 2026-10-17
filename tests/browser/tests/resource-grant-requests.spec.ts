import { fixtureVersionId, fixtureDraftIdKnown } from '../draft-fixtures'
import { expect, type Browser, type BrowserContext, type Locator, type Page, test } from '@playwright/test'

const adminPassword = 'agentx-e2e-admin-password'
const editorPassword = 'agentx-resource-editor-password'
const reviewerPassword = 'agentx-resource-reviewer-password'
const credentialDepartment = 'Credential Governance'
const modelDepartment = 'Model Governance'
const modelName = 'Cross Department Model'
const echoBaseUrl = process.env.AGENTX_E2E_ECHO_BASE_URL ?? 'http://echo-mcp:8090'

async function dialog(page: Page, title: string | RegExp) {
  const value = page.getByRole('dialog', { name: title })
  await expect(value).toBeVisible()
  return value
}

async function field(container: Locator, label: string) {
  return container.locator('label').filter({ hasText: label }).locator('input, textarea').first()
}

async function select(container: Locator, label: string, option: string) {
  await container.getByRole('combobox', { name: label, exact: true }).click()
  await container.page().getByRole('option', { name: option, exact: true }).click()
}

async function submit(container: Locator, name = '保存') {
  await container.getByRole('button', { name, exact: true }).click()
  await expect(container).toBeHidden()
}

async function login(page: Page, username: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill(username)
  await page.getByLabel('密码').fill(password)
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
}

async function loginNewUser(browser: Browser, origin: string, username: string, password: string) {
  const context = await browser.newContext({ baseURL: origin })
  const page = await context.newPage()
  await page.goto('/login')
  await page.getByLabel('用户名').fill(username)
  await page.getByLabel('密码').fill('123456')
  await page.getByRole('button', { name: '登录' }).click()
  await page.getByLabel('新密码', { exact: true }).fill(password)
  await page.getByLabel('确认新密码', { exact: true }).fill(password)
  await page.getByRole('button', { name: '更新密码并登录' }).click()
  await expect(page).toHaveURL(/\/$/)
  return { context, page }
}

async function createEditorRole(page: Page) {
  await page.goto('/roles')
  await page.getByRole('button', { name: '新建角色' }).click()
  const form = await dialog(page, '新建角色')
  await (await field(form, '名称')).fill('Workflow Resource Editor')
  await (await field(form, '角色标识')).fill('workflow_resource_editor')
  await select(form, '数据范围', '全公司')
  for (const permission of [
    'department:view', 'workflow:view', 'workflow:create', 'workflow:edit', 'workflow:archive',
    'workflow:publish', 'credential:view', 'model:view', 'approval:view', 'notification:view',
    'execution:view', 'execution:run', 'trace:view',
  ]) {
    await form.locator('label').filter({ hasText: permission }).getByRole('checkbox').check()
  }
  await submit(form)
  await page.getByRole('searchbox', { name: '搜索角色' }).fill('Workflow Resource Editor')
  await expect(page.getByRole('row').filter({ hasText: 'Workflow Resource Editor' })).toBeVisible()
}

async function createDepartment(page: Page, name: string) {
  await page.goto('/organization')
  await page.getByRole('button', { name: '新建部门' }).click()
  const form = await dialog(page, /新建部门/)
  await (await field(form, '名称')).fill(name)
  await submit(form)
  await expect(page.getByText(name, { exact: true })).toBeVisible()
}

async function createUser(page: Page, username: string, displayName: string, department: string, role: string) {
  await page.goto('/organization')
  await page.getByRole('button', { name: '创建用户' }).click()
  const form = await dialog(page, '创建用户')
  await (await field(form, '用户名')).fill(username)
  await (await field(form, '用户名称')).fill(displayName)
  await select(form, '部门', department)
  await select(form, '角色', role)
  await submit(form)
  await expect(page.getByRole('row').filter({ hasText: username })).toBeVisible()
}

async function createCrossDepartmentModel(page: Page) {
  await page.goto('/credentials')
  await page.getByRole('button', { name: '新建凭证' }).click()
  const credential = await dialog(page, '新建凭证')
  await (await field(credential, '名称')).fill('Cross Department Credential')
  await select(credential, '凭证类型', 'Bearer Token')
  await (await field(credential, '密钥内容')).fill('m5-model-secret')
  await select(credential, '所属部门', credentialDepartment)
  await submit(credential)

  await page.goto('/models')
  await page.getByRole('button', { name: '新建模型' }).click()
  const model = await dialog(page, '新建模型')
  await (await field(model, '连接名称')).fill('Cross Department Connection')
  await (await field(model, 'Endpoint')).fill(`${echoBaseUrl}/v1`)
  await select(model, '凭证', 'Cross Department Credential')
  await (await field(model, '模型名称')).fill(modelName)
  await (await field(model, '上游模型 ID')).fill('cross-department-echo')
  await (await field(model, '输入单价/百万 Token')).fill('0.50')
  await (await field(model, '输出单价/百万 Token')).fill('0.80')
  await select(model, '所属部门', modelDepartment)
  await submit(model)
  await expect(page.getByRole('row').filter({ hasText: modelName })).toBeVisible()
}

async function createWorkflow(page: Page, name: string) {
  await page.goto('/workflows')
  await page.getByRole('button', { name: '新建工作流' }).click()
  const form = await dialog(page, '新建工作流')
  await (await field(form, '工作流名称')).fill(name)
  await submit(form)
  await expect(page.getByRole('heading', { name })).toBeVisible()
  const match = page.url().match(/\/workflows\/([^/]+)/)
  if (!match) throw new Error('Created workflow id is missing from the URL')
  return match[1]
}

async function addModelAndRequest(page: Page, workflowId: string, note: string) {
  await page.goto(`/workflows/${workflowId}/versions/${await fixtureVersionId(page, workflowId)}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  const initialEdge = page.locator('.react-flow__edge').first()
  await initialEdge.click({ force: true })
  await page.keyboard.press('Delete')
  await expect(page.locator('.react-flow__edge')).toHaveCount(0)
  await expect(page.getByTestId('node-creator')).toBeVisible()
  await page.getByRole('textbox', { name: '搜索节点' }).fill('model')
  await page.getByTestId('palette-action-model').click()
  const modelNode = page.locator('.react-flow__node.selected').first()
  await expect(modelNode).toBeVisible()
  const picker = page.getByTestId('resource-selector-model').getByRole('combobox')
  await picker.click()
  const option = page.getByRole('option', { name: new RegExp(modelName, 'i') })
  await expect(option).toBeDisabled()
  await option.locator('..').getByRole('button', { name: '申请' }).click()
  const request = await dialog(page, '申请资源授权')
  await request.getByLabel('申请说明').fill(note)
  await request.getByRole('button', { name: '提交申请' }).click()
  await expect(request).toBeHidden()
  return { modelNode, picker }
}

async function authorizeModelWithoutSelecting(page: Page, workflowId: string) {
  await page.goto(`/workflows/${workflowId}/versions/${await fixtureVersionId(page, workflowId)}/editor`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  const initialEdge = page.locator('.react-flow__edge').first()
  await initialEdge.click({ force: true })
  await page.keyboard.press('Delete')
  await expect(page.locator('.react-flow__edge')).toHaveCount(0)
  await expect(page.getByTestId('node-creator')).toBeVisible()
  await page.getByRole('textbox', { name: '搜索节点' }).fill('model')
  await page.getByTestId('palette-action-model').click()
  const picker = page.getByTestId('resource-selector-model').getByRole('combobox')
  await picker.click()
  const option = page.getByRole('option', { name: new RegExp(modelName, 'i') })
  await option.locator('..').getByRole('button', { name: '授权', exact: true }).click()
  const confirmation = await dialog(page, '授权工作流资源')
  await expect(confirmation.getByText('Cross Department Credential', { exact: true })).toBeVisible()
  await confirmation.getByRole('button', { name: '确认授权' }).click()
  await expect(confirmation).toBeHidden()
  await expect(picker).toHaveAccessibleName('选择资源')
  await picker.click()
  await expect(page.getByRole('option', { name: new RegExp(modelName, 'i') })).toBeEnabled()
}

async function openRequest(page: Page, workflowName: string) {
  await page.goto('/approvals?tab=resource-grants')
  await expect(page.getByRole('tab', { name: '资源授权' })).toHaveAttribute('data-state', 'active')
  const row = page.getByRole('row').filter({ hasText: workflowName })
  await expect(row).toBeVisible()
  await row.getByRole('link', { name: '处理' }).click()
  await expect(page.getByText(workflowName, { exact: false }).first()).toBeVisible()
}

async function reviewOwnDepartment(page: Page, action: '通过' | '拒绝') {
  await page.getByRole('button', { name: action, exact: true }).click()
  const confirmation = await dialog(page, action)
  await confirmation.getByRole('button', { name: action, exact: true }).click()
  await expect(confirmation).toBeHidden()
}

async function connect(page: Page, source: Locator, target: Locator) {
  const sourceBox = await source.boundingBox()
  const targetBox = await target.boundingBox()
  if (!sourceBox || !targetBox) throw new Error('Workflow handles are not visible')
  await page.mouse.move(sourceBox.x + sourceBox.width / 2, sourceBox.y + sourceBox.height / 2)
  await page.mouse.down()
  await page.mouse.move(targetBox.x + targetBox.width / 2, targetBox.y + targetBox.height / 2, { steps: 12 })
  await page.mouse.up()
}

async function selectSaveVersionAndRun(page: Page, workflowId: string, modelNode: Locator, picker: Locator) {
  await picker.click()
  const option = page.getByRole('option', { name: new RegExp(modelName, 'i') })
  await expect(option).toBeEnabled({ timeout: 20_000 })
  await option.click()
  await page.getByTestId('parameter-prompt').getByRole('textbox').fill('Return a short E2E response')
  await page.getByRole('button', { name: /^(适应画布|Fit View)$/ }).click()
  await connect(page, page.getByTestId('workflow-start').locator('.react-flow__handle.source[data-handleid="main"]'), modelNode.locator('.react-flow__handle.target[data-handleid="main"]'))
  await connect(page, modelNode.locator('.react-flow__handle.source[data-handleid="main"]'), page.getByTestId('exit-node-exit').locator('.react-flow__handle.target[data-handleid="main"]'))
  await expect(page.locator('.react-flow__edge')).toHaveCount(2)
  const saved = page.waitForResponse((response) => response.request().method() === 'PUT' && response.url().endsWith(`/workflows/${workflowId}/drafts/${fixtureDraftIdKnown(workflowId)}`))
  await page.getByRole('button', { name: '保存', exact: true }).click()
  expect((await saved).status()).toBe(200)
  const started=page.waitForResponse(response=>response.request().method()==='POST'&&response.url().endsWith('/debug-executions'))
  await page.locator('header').getByRole('button',{name:'运行',exact:true}).click()
  const run=await started
  expect(run.status(),await run.text()).toBe(202)
  await page.getByRole('link', { name: '返回', exact: true }).click()
  await page.getByTestId('workflow-version-1').getByRole('button',{name:'发布',exact:true}).click()
  const publish=page.getByRole('dialog',{name:'发布到环境…'})
  await publish.getByRole('button',{name:'发布到环境…',exact:true}).click()
  await expect(publish).not.toBeVisible()
  await expect(page.getByTestId('workflow-version-1')).toContainText('已发布')

}

test('canvas resource grants complete cross-department approval, execution, and rejection flows', async ({ browser, page }) => {
  test.setTimeout(240_000)
  await login(page, 'admin', adminPassword)
  const origin = new URL(page.url()).origin
  await createEditorRole(page)
  await createDepartment(page, credentialDepartment)
  await createDepartment(page, modelDepartment)
  await createUser(page, 'resource.editor', 'Resource Editor', 'Agentx E2E', 'Workflow Resource Editor')
  await createUser(page, 'credential.reviewer', 'Credential Reviewer', credentialDepartment, '部门管理员')
  await createUser(page, 'model.reviewer', 'Model Reviewer', modelDepartment, '部门管理员')
  await createCrossDepartmentModel(page)

  const directWorkflow = await createWorkflow(page, `Direct Resource Authorization ${Date.now()}`)
  await authorizeModelWithoutSelecting(page, directWorkflow)

  const editor = await loginNewUser(browser, origin, 'resource.editor', editorPassword)
  const credentialReviewer = await loginNewUser(browser, origin, 'credential.reviewer', reviewerPassword)
  const modelReviewer = await loginNewUser(browser, origin, 'model.reviewer', reviewerPassword)
  const contexts: BrowserContext[] = [editor.context, credentialReviewer.context, modelReviewer.context]
  try {
    const approvedName = `Cross Department Approved ${Date.now()}`
    const approvedWorkflow = await createWorkflow(editor.page, approvedName)
    const canvas = await addModelAndRequest(editor.page, approvedWorkflow, 'The model node needs the governed model and credential package')

    await modelReviewer.page.goto('/notifications')
    const notification = modelReviewer.page.getByRole('button').filter({ hasText: '有新的资源授权申请' }).first()
    await expect(notification).toBeVisible()
    await notification.click()
    await expect(modelReviewer.page.getByText(approvedName, { exact: false }).first()).toBeVisible()
    await expect(modelReviewer.page.getByText(new RegExp(`^${modelName}$`, 'i')).first()).toBeVisible()
    await expect(modelReviewer.page.getByText('受控依赖', { exact: true })).toBeVisible()
    await reviewOwnDepartment(modelReviewer.page, '通过')
    await expect(modelReviewer.page.getByText('Model Reviewer', { exact: true }).first()).toBeVisible()

    await openRequest(credentialReviewer.page, approvedName)
    await expect(credentialReviewer.page.getByText(new RegExp(`^${modelName}$`, 'i'))).toHaveCount(0)
    await expect(credentialReviewer.page.getByText('Cross Department Credential', { exact: true })).toBeVisible()
    await reviewOwnDepartment(credentialReviewer.page, '通过')
    await expect(credentialReviewer.page.getByText('已通过', { exact: true }).first()).toBeVisible()

    await selectSaveVersionAndRun(editor.page, approvedWorkflow, canvas.modelNode, canvas.picker)

    const rejectedName = `Cross Department Rejected ${Date.now()}`
    const rejectedWorkflow = await createWorkflow(editor.page, rejectedName)
    const rejectedCanvas = await addModelAndRequest(editor.page, rejectedWorkflow, 'Exercise the rejection and retry state')
    await openRequest(modelReviewer.page, rejectedName)
    await reviewOwnDepartment(modelReviewer.page, '拒绝')
    await expect(modelReviewer.page.getByText('已拒绝', { exact: true }).first()).toBeVisible()

    await rejectedCanvas.picker.click()
    const rejectedOption = editor.page.getByRole('option', { name: new RegExp(modelName, 'i') })
    await expect(rejectedOption.locator('..').getByRole('button', { name: '重新申请' })).toBeVisible({ timeout: 20_000 })
  } finally {
    await Promise.all(contexts.map((context) => context.close()))
  }
})
