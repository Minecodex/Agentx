import { readFile } from 'node:fs/promises'

import { expect, type APIResponse, type Page, test } from '@playwright/test'

const password = 'agentx-e2e-admin-password'

type RuntimeContext = { workflowId: string; workflowVersionId: string }
type Environment = { id: string; code: string }
type Deployment = { id: string; status: string }

async function expectResponse(response: APIResponse, label: string) {
  if (!response.ok()) throw new Error(`${label}: ${response.status()} ${await response.text()}`)
}

async function request<T>(page: Page, token: string, path: string, method = 'GET', body?: unknown) {
  const response = await page.request.fetch(`/api/v1${path}`, {
    method,
    data: body,
    headers: { Authorization: `Bearer ${token}` },
  })
  await expectResponse(response, `${method} ${path}`)
  const text = await response.text()
  return (text ? JSON.parse(text) : undefined) as T
}

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill(password)
  const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/auth/login') && value.request().method() === 'POST')
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
  return ((await (await response).json()) as { accessToken: string }).accessToken
}

test('channel dialog renders provider-specific fields and stream channels hide the endpoint', async ({ page }) => {
  const contextPath = process.env.AGENTX_V2_08_CONTEXT_OUTPUT
  if (!contextPath) throw new Error('AGENTX_V2_08_CONTEXT_OUTPUT is required')
  const context = JSON.parse(await readFile(contextPath, 'utf8')) as RuntimeContext
  const token = await login(page)
  const environment = (await request<Environment[]>(page, token, '/environments')).find((item) => item.code === 'development')
  expect(environment).toBeTruthy()
  const suffix = Date.now()
  const application = await request<{ id: string }>(page, token, '/applications', 'POST', {
    workflowId: context.workflowId,
    name: `Channel Forms ${suffix}`,
    slug: `channel-forms-${suffix}`,
    description: 'Channel dynamic form coverage',
    visibility: 'company',
  })
  const deployment = await request<Deployment>(page, token, `/applications/${application.id}/deployments`, 'POST', {
    workflowVersionId: context.workflowVersionId,
    environmentId: environment!.id,
    sessionVersionPolicy: 'pinned',
  })
  await expect.poll(async () => {
    const values = await request<Deployment[]>(page, token, `/applications/${application.id}/deployments`)
    const status = values.find((item) => item.id === deployment.id)?.status
    if (status === 'rejected') throw new Error(`application deployment ${deployment.id} was rejected`)
    return status
  }, { timeout: 180_000, intervals: [500, 1_000, 2_000] }).toBe('active')

  await page.goto(`/applications/${application.id}`)
  await page.getByRole('tab', { name: '渠道对接' }).click()
  await page.getByRole('button', { name: '新增渠道' }).click()
  const dialog = page.getByRole('dialog', { name: 'Webhook' })

  // DingTalk callback defaults: signing secret plus optional AES key, mode selectable.
  await expect(dialog.getByLabel('签名 Secret')).toBeVisible()
  await expect(dialog.getByLabel('加解密 AES Key（可选）')).toBeVisible()
  await expect(dialog.getByLabel('接入模式')).toBeVisible()

  // WeCom has no stream mode on the platform: mode select disappears.
  await dialog.getByLabel('平台').click()
  await page.getByRole('option', { name: '企业微信' }).click()
  await expect(dialog.getByLabel('接入模式')).toHaveCount(0)
  await expect(dialog.getByLabel('校验 Token')).toBeVisible()
  await expect(dialog.getByLabel('EncodingAESKey')).toBeVisible()

  // Feishu stream swaps in App ID / App Secret.
  await dialog.getByLabel('平台').click()
  await page.getByRole('option', { name: '飞书' }).click()
  await dialog.getByLabel('接入模式').click()
  await page.getByRole('option', { name: '长连接 (Stream)' }).click()
  await expect(dialog.getByLabel('App ID')).toBeVisible()
  await expect(dialog.getByLabel('App Secret')).toBeVisible()
  await expect(dialog.getByLabel('Verification Token')).toHaveCount(0)

  // DingTalk stream swaps in Client ID / Client Secret.
  await dialog.getByLabel('平台').click()
  await page.getByRole('option', { name: '钉钉' }).click()
  await dialog.getByLabel('接入模式').click()
  await page.getByRole('option', { name: '长连接 (Stream)' }).click()
  await expect(dialog.getByLabel('Client ID (AppKey)')).toBeVisible()
  await expect(dialog.getByLabel('Client Secret')).toBeVisible()
  await expect(dialog.getByLabel('签名 Secret')).toHaveCount(0)

  await dialog.getByLabel('名称', { exact: true }).fill(`钉钉长连接渠道 ${suffix}`)
  await dialog.getByLabel('Client ID (AppKey)').fill('ding-e2e-client-id')
  await dialog.getByLabel('Client Secret').fill('ding-e2e-client-secret')
  await dialog.getByRole('combobox', { name: '来源字段', exact: true }).click()
  await page.getByRole('option', { name: '消息内容', exact: true }).click()
  await dialog.getByRole('button', { name: '保存' }).click()
  await expect(dialog).toHaveCount(0)

  const channel = page.getByRole('button', { name: new RegExp(`^钉钉长连接渠道 ${suffix}`) }).first()
  await expect(channel).toBeVisible()
  await expect(channel.getByText(/钉钉 · 长连接 \(Stream\)/)).toBeVisible()
  await expect(page.getByText('连接状态', { exact: true })).toBeVisible()
  await expect(page.getByText('生产接入地址', { exact: true })).toHaveCount(0)
  await expect(page.getByText(/长连接模式无需公网接入地址/)).toBeVisible()

  // Missing required config fields are rejected server-side per the template.
  const rejected = await page.request.fetch(`/api/v1/applications/${application.id}/webhooks`, {
    method: 'POST',
    data: { name: `缺字段 ${suffix}`, providerType: 'dingtalk', channelMode: 'callback', channelConfig: {} },
    headers: { Authorization: `Bearer ${token}` },
  })
  expect(rejected.status()).toBe(422)
})
