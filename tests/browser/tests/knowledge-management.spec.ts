import { expect, type Page, test } from '@playwright/test'

const password = 'agentx-e2e-admin-password'

async function login(page: Page) {
  const bootstrapStatus = await page.request.get('/api/v1/bootstrap/status')
  if (!bootstrapStatus.ok()) throw new Error(`bootstrap status: ${bootstrapStatus.status()}`)
  if (((await bootstrapStatus.json()) as { required: boolean }).required) {
    await page.goto('/setup')
    await page.getByLabel('公司名称').fill('Agentx E2E')
    await page.getByLabel('用户名').fill('admin')
    await page.getByLabel('管理员姓名').fill('E2E Admin')
    await page.getByLabel('密码').fill(password)
    const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/bootstrap') && value.request().method() === 'POST')
    await page.getByRole('button', { name: '初始化并进入工作台' }).click()
    await expect(page).toHaveURL(/\/$/)
    return ((await (await response).json()) as { accessToken: string }).accessToken
  }
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill(password)
  const response = page.waitForResponse((value) => value.url().endsWith('/api/v1/auth/login') && value.request().method() === 'POST')
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/$/)
  return ((await (await response).json()) as { accessToken: string }).accessToken
}

async function choose(page: Page, label: string, value: string) {
  await page.getByRole('dialog').getByRole('combobox', { name: label, exact: true }).click()
  await page.getByRole('option', { name: value, exact: true }).click()
}

for (const provider of ['ragflow', 'lightrag'] as const) test(`${provider} connection and query CRUD, grants, external documents and retrieval dialog`, async ({ page }, info) => {
  const token = await login(page)
  const headers = { Authorization: `Bearer ${token}` }
  const departments = await (await page.request.get('/api/v1/departments', { headers })).json()
  const name = `Knowledge ${provider} ${Date.now()}`
  await page.goto('/knowledge')
  await page.getByRole('button', { name: '新建连接', exact: true }).click()
  let form = page.getByRole('dialog')
  await form.getByLabel('连接名称').fill(name)
  await choose(page, '服务协议', provider === 'ragflow' ? 'RAGFlow' : 'LightRAG')
  await form.getByLabel('服务地址').fill(process.env.AGENTX_E2E_KNOWLEDGE_ENDPOINT!)
  await choose(page, '所属部门', departments[0].name)
  await expect(form.getByText('资源名称', { exact: true })).toHaveCount(0)
  await expect(form.getByText('外部资源 ID', { exact: true })).toHaveCount(0)
  await form.getByRole('button', { name: '保存', exact: true }).click()
  await expect(form).toBeHidden()
  const row = page.getByRole('row').filter({ hasText: name })
  await row.getByRole('button', { name: '测试连接', exact: true }).click()
  await expect(page.getByText(/^连接正常 ·/)).toBeVisible()
  await row.getByRole('link', { name: '详情', exact: true }).click()
  const connectionId = page.url().split('/').at(-1)!
  await page.getByRole('button', { name: '新建查询', exact: true }).click()
  form = page.getByRole('dialog')
  await form.getByLabel('查询名称').fill(`${name} query`)
  await form.getByLabel('用途说明').fill('Launch and support questions')
  if (provider === 'ragflow') {
    await form.getByRole('checkbox', { name: /Product documentation/ }).check()
    await form.getByRole('checkbox', { name: /Support handbook/ }).check()
  } else await choose(page, '工作区', '默认工作区')
  await form.getByRole('button', { name: '保存', exact: true }).click()
  const grants = page.getByRole('dialog', { name: `管理“${name} query”的授权`, exact: true })
  await expect(grants).toBeVisible()
  await choose(page, '授权对象类型', '部门')
  await choose(page, '授权对象', departments[0].name)
  await grants.getByRole('button', { name: '添加授权', exact: true }).click()
  await expect(grants.getByRole('button', { name: '撤销授权', exact: true })).toBeVisible()
  await grants.getByRole('button', { name: '取消', exact: true }).click()
  await expect(page.getByRole('heading', { name: `${name} query`, exact: true })).toBeVisible()
  await expect(page.getByText(/1 条授权/)).toBeVisible()
  const queries = await (await page.request.get(`/api/v1/knowledge/connections/${connectionId}/queries`, { headers })).json()
  const query = queries.items[0]
  expect(query.scope).toEqual(provider === 'ragflow' ? { kind: 'ragflow', datasetIds: ['dataset-a', 'dataset-b'] } : { kind: 'lightrag', workspace: '' })
  const protectedDelete = await page.request.delete(`/api/v1/knowledge/connections/${connectionId}`, { headers })
  expect(protectedDelete.status()).toBe(409)
  await page.getByRole('button', { name: '外部文档', exact: true }).click()
  await expect(page.getByRole('dialog')).toContainText('Launch guide')
  await expect(page.getByRole('dialog').getByRole('button', { name: /上传|删除/ })).toHaveCount(0)
  await page.getByRole('dialog').getByRole('button', { name: '取消', exact: true }).click()
  await page.getByRole('button', { name: '检索测试', exact: true }).click()
  form = page.getByRole('dialog')
  for (const text of [provider === 'ragflow' ? 'fixture-partial' : 'launch code', 'fixture-empty', 'fixture-error']) {
    await form.getByPlaceholder('输入查询内容验证检索').fill(text)
    await form.getByRole('button', { name: '执行', exact: true }).click()
    if (text === 'fixture-empty') await expect(form).toContainText('没有命中片段')
    else if (text === 'fixture-error') await expect(form.getByRole('alert')).toBeVisible()
    else {
      await expect(form).toContainText('sapphire-orbit')
      if (provider === 'ragflow') await expect(form).toContainText('部分来源失败')
    }
    await page.screenshot({ path: info.outputPath(`${provider}-${text}.png`), fullPage: true })
  }
  await form.getByRole('button', { name: '取消', exact: true }).click()
  await page.getByRole('button', { name: '编辑', exact: true }).click()
  form = page.getByRole('dialog')
  await form.getByLabel('用途说明').fill('Updated query purpose')
  await form.getByRole('checkbox', { name: /保存后配置授权/ }).uncheck()
  await form.getByRole('button', { name: '保存', exact: true }).click()
  await expect(form).toBeHidden()
  await expect(page.getByText('Updated query purpose', { exact: true })).toBeVisible()
  for (const language of ['English', '简体中文']) {
    await page.getByRole('button', { name: /^(语言|Language)$/ }).click()
    await page.getByRole('menuitemradio', { name: language, exact: true }).click()
    for (const dark of [true, false]) {
      await page.getByRole('button', { name: /^(主题|Theme)$/ }).click()
      await page.getByRole('menuitemradio', { name: language === 'English' ? dark ? 'Dark' : 'Light' : dark ? '深色' : '浅色', exact: true }).click()
      await page.screenshot({ path: info.outputPath(`${provider}-${language}-${dark}.png`), fullPage: true })
    }
  }
  await page.getByRole('button', { name: '编辑连接', exact: true }).click()
  await choose(page, '状态', '停用')
  await page.getByRole('dialog').getByRole('button', { name: '保存', exact: true }).click()
  await expect(page.getByRole('button', { name: '检索测试', exact: true })).toBeDisabled()
  const removed = await page.request.delete(`/api/v1/knowledge/queries/${query.id}`, { headers })
  expect(removed.status()).toBe(204)
  await page.goto('/knowledge')
  await page.getByRole('row').filter({ hasText: name }).getByRole('button', { name: '删除', exact: true }).click()
  await page.getByRole('dialog').getByRole('button', { name: '删除', exact: true }).click()
  await expect(page.getByRole('row').filter({ hasText: name })).toHaveCount(0)
})
