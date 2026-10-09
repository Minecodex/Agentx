import { createHmac } from 'node:crypto'
import { expect, type Locator, type Page, test } from '@playwright/test'

const runId = process.env.AGENTX_E2E_RUN_ID ?? String(Date.now())
const secret = 'agentx-e2e-channel-placeholder'
const template = '回答：{{output.result}}\n发送者：{{output.requester}}'

async function api(page: Page, token: string, path: string, method = 'GET', data?: unknown) {
  const response = await page.request.fetch(`/api/v1${path}`, { method, data, headers: { Authorization: `Bearer ${token}` } })
  expect(response.ok(), `${method} ${path}: ${await response.text()}`).toBeTruthy()
  return response.json()
}

async function select(page: Page, control: Locator, option: string) {
  await control.click()
  await page.getByRole('option', { name: option, exact: true }).click()
}

async function expectBelow(lower: Locator, upper: Locator) {
  const upperBox = await upper.boundingBox()
  const lowerBox = await lower.boundingBox()
  expect(upperBox).not.toBeNull()
  expect(lowerBox).not.toBeNull()
  expect(lowerBox!.y).toBeGreaterThanOrEqual(upperBox!.y + upperBox!.height)
}

async function fixture(page: Page, token: string) {
  const workflow = await api(page, token, '/workflows', 'POST', { name: `Channel form ${runId}`, visibility: 'company' })
  const draft = await api(page, token, `/workflows/${workflow.id}/draft`)
  const definition = draft.definition
  definition.start.inputs = { type: 'object', properties: { question: { type: 'string' }, sender: { type: 'string' }, context: { type: 'string' }, limit: { type: 'integer' } }, required: ['question', 'sender'], additionalProperties: false }
  const exit = definition.nodes.find((node: { type: string }) => node.type === 'exit')
  const reference = (name: string) => ({ kind: 'reference', selector: { namespace: 'inputs', run: { kind: 'current' }, item: { kind: 'current' }, path: [name] }, missingPolicy: { kind: 'error' } })
  exit.parameters.outputs = { result: reference('question'), requester: reference('sender') }
  definition.end.outputs = { result: { schema: { type: 'string' }, required: true, sensitive: false }, requester: { schema: { type: 'string' }, required: true, sensitive: false } }
  definition.connections = [{ id: 'start-exit', sourceNodeId: '__start__', sourceHandle: 'main', targetNodeId: exit.id, targetHandle: 'main', order: 0 }]
  await api(page, token, `/workflows/${workflow.id}/draft`, 'PUT', { expectedRevision: draft.revision, definition })
  const latest = await api(page, token, `/workflows/${workflow.id}/draft`)
  const version = await api(page, token, `/workflows/${workflow.id}/versions`, 'POST', { draftRevision: latest.revision })
  const environments = await api(page, token, '/environments')
  const environmentId = environments.find((item: { code: string }) => item.code === 'development').id
  await api(page, token, `/workflows/${workflow.id}/deployments`, 'POST', { environmentId, workflowVersionId: version.id })
  return { workflow, version, environmentId }
}

test('configures replies and required/optional mappings before deployment, edits, publishes and delivers', async ({ page }, testInfo) => {
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill('agentx-e2e-admin-password')
  const loggedIn = page.waitForResponse((response) => response.url().endsWith('/api/v1/auth/login') && response.request().method() === 'POST')
  await page.getByRole('button', { name: '登录', exact: true }).click()
  const token = (await (await loggedIn).json()).accessToken
  await expect(page).toHaveURL(/\/$/)
  const { workflow, version, environmentId } = await fixture(page, token)

  await page.getByRole('link', { name: '应用', exact: true }).click()
  await page.getByRole('button', { name: '新建应用', exact: true }).click()
  const create = page.getByRole('dialog', { name: '新建应用', exact: true })
  await select(page, create.getByRole('combobox', { name: '工作流', exact: true }), workflow.name)
  await create.getByLabel('名称', { exact: true }).fill(`Channel app ${runId}`)
  await create.getByLabel('Slug', { exact: true }).fill(`channel-${runId}`)
  const created = page.waitForResponse((response) => response.url().endsWith('/api/v1/applications') && response.request().method() === 'POST')
  await create.getByRole('button', { name: '保存', exact: true }).click()
  const application = await (await created).json()
  await expect(page).toHaveURL(new RegExp(`/applications/${application.id}$`))
  expect(await api(page, token, `/applications/${application.id}/deployments`)).toEqual([])
  await page.getByRole('tab', { name: '渠道对接', exact: true }).click()
  await expect(page.getByRole('button', { name: '新增渠道', exact: true })).toBeEnabled()

  const requiredMappings = [{ target: 'question', source: 'message.text', missingPolicy: 'error' }, { target: 'sender', source: 'sender.name', missingPolicy: 'error' }]
  await page.getByRole('button', { name: '新增渠道', exact: true }).click()
  const streamForm = page.getByRole('dialog', { name: 'Webhook', exact: true })
  await streamForm.getByLabel('名称', { exact: true }).fill('Unpublished Feishu stream')
  await select(page, streamForm.getByRole('combobox', { name: '平台', exact: true }), '飞书')
  await select(page, streamForm.getByRole('combobox', { name: '接入模式', exact: true }), '长连接 (Stream)')
  await streamForm.getByLabel('App ID', { exact: true }).fill('cli_e2e_placeholder')
  await streamForm.getByLabel('App Secret', { exact: true }).fill(secret)
  await select(page, streamForm.getByRole('combobox', { name: '来源字段', exact: true }).nth(0), '消息内容')
  await select(page, streamForm.getByRole('combobox', { name: '来源字段', exact: true }).nth(1), '发送者昵称')
  await streamForm.getByRole('button', { name: '保存', exact: true }).click()
  await expect(streamForm).toBeHidden()
  const connection = page.getByRole('group', { name: '连接状态', exact: true })
  await expect(connection.getByText('待发布', { exact: true })).toBeVisible()
  await expect(connection.getByText('等待连接', { exact: true })).toHaveCount(0)
  await expect(connection.getByText('渠道配置尚未发布，长连接还未启动。请创建应用部署，发布成功后系统会自动连接平台。', { exact: true })).toBeVisible()
  await connection.getByRole('button', { name: '创建部署', exact: true }).click()
  const streamDeployment = page.getByRole('dialog', { name: '部署', exact: true })
  await expect(streamDeployment).toBeVisible()
  await streamDeployment.getByRole('button', { name: '取消', exact: true }).click()
  await page.screenshot({ path: testInfo.outputPath('channel-stream-unpublished.png'), fullPage: true })
  await page.getByRole('button', { name: '删除', exact: true }).click()
  const removeStream = page.getByRole('dialog', { name: '删除 Unpublished Feishu stream', exact: true })
  await removeStream.getByRole('button', { name: '确认删除', exact: true }).click()
  await expect(removeStream).toBeHidden()
  await expect(connection).toHaveCount(0)
  const invalidBase = { name: 'Invalid channel', providerType: 'dingtalk', channelMode: 'callback', channelConfig: { secret }, fixedInputs: {} }
  for (const [data, code] of [
    [{ ...invalidBase, inputMappings: requiredMappings.slice(0, 1) }, 'WEBHOOK_REQUIRED_INPUT_UNMAPPED'],
    [{ ...invalidBase, inputMappings: [...requiredMappings, { target: 'unknown', source: 'message.text', missingPolicy: 'error' }] }, 'WEBHOOK_MAPPING_TARGET_UNKNOWN'],
    [{ ...invalidBase, inputMappings: requiredMappings, reply: { enabled: true, outputField: 'unknown' } }, 'WEBHOOK_REPLY_OUTPUT_FIELD_UNKNOWN'],
  ] as const) {
    const response = await page.request.post(`/api/v1/applications/${application.id}/webhooks`, { headers: { Authorization: `Bearer ${token}` }, data })
    expect(response.status()).toBe(422)
    expect((await response.json()).code).toBe(code)
  }

  await page.getByRole('button', { name: '新增渠道', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Webhook', exact: true })
  await expect(dialog.getByRole('combobox', { name: '状态', exact: true })).toHaveCount(0)
  await dialog.getByLabel('名称', { exact: true }).fill('DingTalk form channel')
  await select(page, dialog.getByRole('combobox', { name: '平台', exact: true }), '钉钉')
  await dialog.getByLabel('签名 Secret', { exact: true }).fill(secret)
  await expect(dialog.getByRole('combobox', { name: '回复输出字段', exact: true })).toHaveCount(0)
  await expect(dialog.getByRole('textbox', { name: '回复模板', exact: true })).toHaveCount(0)
  await expect(dialog.getByRole('combobox', { name: '工作流输入', exact: true })).toHaveCount(2)
  await expect(dialog.getByRole('combobox', { name: '工作流输入', exact: true }).nth(0)).toHaveText('question *')
  await expect(dialog.getByRole('combobox', { name: '工作流输入', exact: true }).nth(1)).toHaveText('sender *')
  await expect(dialog.getByRole('combobox', { name: '来源字段', exact: true }).nth(0)).toHaveText('选择来源字段')

  await select(page, dialog.getByRole('combobox', { name: '开启回复', exact: true }), '开启')
  await expect(dialog.getByRole('textbox', { name: '回复模板', exact: true })).toHaveJSProperty('tagName', 'TEXTAREA')
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  await expect(dialog.getByText('此项为必填项', { exact: true })).toBeVisible()
  await dialog.getByRole('combobox', { name: '回复输出字段', exact: true }).click()
  await expect(page.getByRole('option', { name: 'requester', exact: true })).toBeVisible()
  await page.getByRole('option', { name: 'result', exact: true }).click()
  await dialog.getByRole('textbox', { name: '回复模板', exact: true }).fill(template)
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  await expect(dialog.getByText('请为输入 question 选择来源字段。', { exact: true })).toBeVisible()
  await select(page, dialog.getByRole('combobox', { name: '来源字段', exact: true }).nth(0), '消息内容')
  await select(page, dialog.getByRole('combobox', { name: '来源字段', exact: true }).nth(1), '发送者昵称')
  await dialog.getByRole('button', { name: '添加映射', exact: true }).click()
  const optional = dialog.getByRole('combobox', { name: '工作流输入', exact: true }).nth(2)
  await optional.click()
  for (const name of ['question *', 'sender *', 'context', 'limit']) await expect(page.getByRole('option', { name, exact: true })).toBeVisible()
  await page.getByRole('option', { name: 'context', exact: true }).click()
  await select(page, dialog.getByRole('combobox', { name: '来源字段', exact: true }).nth(2), '会话 ID')
  await dialog.getByRole('button', { name: '添加固定输入', exact: true }).click()
  await expect(dialog.getByRole('combobox', { name: '工作流输入', exact: true }).nth(3)).toHaveText('limit')
  await dialog.getByLabel('固定输入值', { exact: true }).fill('4')
  await select(page, dialog.getByRole('combobox', { name: '开启回复', exact: true }), '关闭')
  await expect(dialog.getByRole('textbox', { name: '回复模板', exact: true })).toHaveCount(0)
  await select(page, dialog.getByRole('combobox', { name: '开启回复', exact: true }), '开启')
  await expect(dialog.getByRole('textbox', { name: '回复模板', exact: true })).toHaveValue(template)
  await expectBelow(dialog.getByRole('combobox', { name: '开启回复', exact: true }), dialog.getByRole('button', { name: '添加固定输入', exact: true }))
  await page.screenshot({ path: testInfo.outputPath('channel-form-before-first-deployment.png'), fullPage: true })
  const saved = page.waitForResponse((response) => response.url().endsWith(`/applications/${application.id}/webhooks`) && response.request().method() === 'POST')
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  const savedResponse = await saved
  expect(savedResponse.status()).toBe(201)
  const channel = await savedResponse.json()
  expect(channel.status).toBe('active')
  expect(channel.reply).toEqual({ enabled: true, outputField: 'result', template })
  expect(channel.inputMappings).toHaveLength(3)
  expect(channel.fixedInputs).toEqual({ limit: 4 })
  await expect(dialog).toBeHidden()

  await page.getByRole('button', { name: '编辑', exact: true }).last().click()
  const edit = page.getByRole('dialog', { name: '编辑 Webhook', exact: true })
  await expect(edit.getByRole('combobox', { name: '回复输出字段', exact: true })).toHaveText('result')
  await expect(edit.getByRole('textbox', { name: '回复模板', exact: true })).toHaveValue(template)
  await expect(edit.getByRole('combobox', { name: '工作流输入', exact: true })).toHaveCount(4)
  await expect(edit.getByLabel('签名 Secret', { exact: true })).toHaveValue('')
  await expectBelow(edit.getByRole('combobox', { name: '开启回复', exact: true }), edit.getByRole('button', { name: '添加固定输入', exact: true }))
  await expect(edit.getByRole('combobox', { name: '状态', exact: true })).toHaveCount(0)
  await page.screenshot({ path: testInfo.outputPath('channel-form-edit-inputs-before-reply.png'), fullPage: true })
  const updated = page.waitForResponse((response) => response.url().endsWith(`/webhooks/${channel.id}`) && response.request().method() === 'PATCH')
  await edit.getByRole('button', { name: '保存', exact: true }).click()
  const updatedResponse = await updated
  expect(updatedResponse.status(), await updatedResponse.text()).toBe(200)
  expect((await updatedResponse.json()).status).toBe('active')
  await expect(edit).toBeHidden()

  const disabled = page.waitForResponse((response) => response.url().endsWith(`/webhooks/${channel.id}`) && response.request().method() === 'PATCH')
  await page.getByRole('button', { name: '停用', exact: true }).click()
  const disabledResponse = await disabled
  expect(disabledResponse.status(), await disabledResponse.text()).toBe(200)
  expect((await disabledResponse.json()).status).toBe('disabled')
  await expect(page.getByRole('button', { name: '启用', exact: true })).toBeVisible()
  await page.getByRole('button', { name: '编辑', exact: true }).last().click()
  await expect(edit.getByRole('combobox', { name: '状态', exact: true })).toHaveCount(0)
  const savedDisabled = page.waitForResponse((response) => response.url().endsWith(`/webhooks/${channel.id}`) && response.request().method() === 'PATCH')
  await edit.getByRole('button', { name: '保存', exact: true }).click()
  const savedDisabledResponse = await savedDisabled
  expect(savedDisabledResponse.status(), await savedDisabledResponse.text()).toBe(200)
  expect((await savedDisabledResponse.json()).status).toBe('disabled')
  await expect(edit).toBeHidden()
  await expect(page.getByRole('button', { name: '启用', exact: true })).toBeVisible()
  const enabled = page.waitForResponse((response) => response.url().endsWith(`/webhooks/${channel.id}`) && response.request().method() === 'PATCH')
  await page.getByRole('button', { name: '启用', exact: true }).click()
  const enabledResponse = await enabled
  expect(enabledResponse.status(), await enabledResponse.text()).toBe(200)
  expect((await enabledResponse.json()).status).toBe('active')
  await expect(page.getByRole('button', { name: '停用', exact: true })).toBeVisible()

  await page.getByRole('tab', { name: '应用部署', exact: true }).click()
  await page.getByRole('button', { name: '创建部署', exact: true }).click()
  const deploy = page.getByRole('dialog', { name: '部署', exact: true })
  await select(page, deploy.getByRole('combobox', { name: '工作流版本', exact: true }), `v${version.versionNumber}`)
  const environments = await api(page, token, '/environments')
  await select(page, deploy.getByRole('combobox', { name: '环境', exact: true }), environments.find((item: { id: string }) => item.id === environmentId).name)
  await deploy.getByRole('button', { name: '保存', exact: true }).click()
  await expect(deploy).toBeHidden()
  await expect.poll(async () => (await api(page, token, `/applications/${application.id}/deployments`))[0]?.status, { timeout: 120_000 }).toBe('active')
  await page.getByRole('tab', { name: '渠道对接', exact: true }).click()
  await expect(page.getByText('已启用 · 已发布', { exact: true })).toBeVisible()

  const timestamp = String(Date.now())
  const signature = createHmac('sha256', secret).update(`${timestamp}\n${secret}`).digest('base64')
  const inbound = `${process.env.AGENTX_E2E_RUNTIME_URL}${channel.path}?timestamp=${timestamp}&sign=${encodeURIComponent(signature)}`
  await expect.poll(async () => {
    const response = await page.request.post(inbound, { data: { msgId: `channel-form-${runId}`, conversationId: 'e2e-form-chat', conversationType: '1', senderId: 'e2e-sender', senderNick: 'E2E', msgtype: 'text', content: JSON.stringify({ content: 'channel form message' }), sessionWebhook: `${process.env.AGENTX_E2E_IM_MOCK_URL}/dingtalk/session-channel-form`, sessionWebhookExpiredTime: Date.now() + 3600_000, createAt: Date.now() } })
    return [200, 202].includes(response.status())
  }, { timeout: 60_000 }).toBe(true)
  await expect.poll(async () => {
    const result = await api(page, token, `/deliveries?applicationId=${application.id}&limit=50`)
    return result.items[0]?.status
  }, { timeout: 120_000 }).toBe('delivered')
  const delivered = await api(page, token, `/deliveries?applicationId=${application.id}&limit=50`)
  expect(delivered.items[0].providerMessageId).toBeTruthy()
  await page.getByRole('tab', { name: '投递记录', exact: true }).click()
  const statusFilter = page.getByRole('combobox', { name: '投递状态', exact: true })
  await expect(statusFilter).toHaveText('全部状态')
  await select(page, statusFilter, '已送达')
  await expect(page.getByText(delivered.items[0].providerMessageId, { exact: true })).toBeVisible()
  await select(page, statusFilter, '死信')
  await expect(page.getByText('暂无投递记录', { exact: true })).toBeVisible()
  await select(page, statusFilter, '全部状态')
  await expect(page.getByText(delivered.items[0].providerMessageId, { exact: true })).toBeVisible()
  await page.screenshot({ path: testInfo.outputPath('channel-delivery-records.png'), fullPage: true })
  await page.getByRole('tab', { name: '渠道对接', exact: true }).click()
  await page.screenshot({ path: testInfo.outputPath('channel-configuration-published.png'), fullPage: true })
})
