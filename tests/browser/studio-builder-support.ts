import { expect, test as base, type Page } from '@playwright/test'

export type Created = { id: string; createdDraftId: string }
export type Draft = { id: string; revision: number; definition: { nodes: Array<{ id: string; name: string; type: string }>; connections: unknown[] }; definitionHash: string }
const activeRuns = new Map<Page,{id:string;session:string;authorization:string}>()

// Failed assertions can leave an ask_user run waiting after its page is closed.
// Cancel only runs created by this test, so they cannot consume the next test's slots.
export const test = base.extend<{ designRunCleanup: void }>({
  designRunCleanup: [async ({ request }, use) => {
    try {
      await use()
    } finally {
      const runs = [...activeRuns.values()]
      activeRuns.clear()
      for (const run of runs) {
        const response = await request.post(`/api/v1/studio-builder/sessions/${run.session}/runs/${run.id}/cancel`, {
          headers: { Authorization: run.authorization },
        })
        // Some security cases deliberately revoke access or delete the draft/session.
        expect([200, 401, 403, 404]).toContain(response.status())
      }
    }
  }, { auto: true }],
})

export async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill('admin')
  await page.getByLabel('密码').fill('agentx-e2e-admin-password')
  const response = page.waitForResponse(value => value.url().endsWith('/api/v1/auth/login') && value.request().method() === 'POST')
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await expect(page).toHaveURL(/\/$/)
  return ((await (await response).json()) as { accessToken: string }).accessToken
}

export async function read<T>(page: Page, token: string, path: string): Promise<T> {
  const response = await page.request.get(`/api/v1${path}`, { headers: { Authorization: `Bearer ${token}` } })
  expect(response.ok(), await response.text()).toBeTruthy()
  return response.json() as Promise<T>
}

export async function allEntries<T>(page: Page, token: string, session: string) {
  const items: T[] = []
  let cursor = 0
  for (;;) {
    const next = await read<{items:T[];nextCursor:number}>(page,token,`/studio-builder/sessions/${session}/entries?after=${cursor}`)
    items.push(...next.items)
    if (next.items.length < 200) return items
    expect(next.nextCursor).toBeGreaterThan(cursor)
    cursor = next.nextCursor
  }
}

export async function setModel(page: Page, alias: string) {
  await page.goto('/models')
  const row = page.getByRole('row').filter({ hasText: alias })
  await expect(row).toBeVisible()
  if (await row.getByRole('button', { name: '设为画布 AI 模型', exact: true }).count()) {
    const saved = page.waitForResponse(value => value.url().endsWith('/studio-builder/model-setting') && value.request().method() === 'PUT')
    await row.getByRole('button', { name: '设为画布 AI 模型', exact: true }).click()
    expect((await saved).status()).toBe(200)
  } else if (await row.getByRole('button',{name:'重新设置',exact:true}).count()) {
    const saved=page.waitForResponse(value=>value.url().endsWith('/studio-builder/model-setting')&&value.request().method()==='PUT')
    await row.getByRole('button',{name:'重新设置',exact:true}).click()
    expect((await saved).status()).toBe(200)
  }
  await expect(row.getByText('画布 AI 模型', { exact: true })).toBeVisible()
}

export async function create(page: Page, name: string) {
  await page.goto('/workflows')
  await page.getByRole('button', { name: '新建工作流', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '新建工作流' })
  await dialog.getByLabel('工作流名称').fill(name)
  const response = page.waitForResponse(value => value.url().endsWith('/api/v1/workflows') && value.request().method() === 'POST')
  await dialog.getByRole('button', { name: '保存', exact: true }).click()
  const received = await response
  expect(received.status(), await received.text()).toBe(201)
  const created = await received.json() as Created
  expect(created.createdDraftId).toMatch(/^[0-9a-f-]{36}$/)
  await expect(page).toHaveURL(new RegExp(`/workflows/${created.id}$`))
  await page.locator(`a[href="/workflows/${created.id}/drafts/${created.createdDraftId}/editor"]`).click()
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  return created
}

export async function count(page: Page, token: string, id: string) {
  const values = await read<{ total: number }>(page, token, `/executions?workflowIds=${id}&pageSize=100`)
  return values.total
}

export async function send(page: Page, prompt: string) {
  const panel = page.getByTestId('studio-builder-panel')
  await panel.getByLabel('描述工作流需求').fill(prompt)
  const started = page.waitForResponse(value => /\/studio-builder\/sessions\/.+\/runs$/.test(new URL(value.url()).pathname) && value.request().method() === 'POST')
  await panel.getByRole('button', { name: '发送', exact: true }).click()
  const response = await started
  expect(response.status(), await response.text()).toBe(200)
  const run = await response.json() as { id: string }
  const session=new URL(response.url()).pathname.split('/').at(-2)!
  activeRuns.set(page,{id:run.id,session,authorization:response.request().headers()['authorization']!})
  return run.id
}

export async function completed(page: Page, requireApplied = true, timeout = 180_000) {
  const panel = page.getByTestId('studio-builder-panel')
  const current=activeRuns.get(page)!
  let status:string|undefined
  try {
    await expect.poll(async()=>{
      const response=await page.request.get(`/api/v1/studio-builder/sessions/${current.session}`,{headers:{Authorization:current.authorization}})
      expect(response.ok(),await response.text()).toBeTruthy()
      const summary=await response.json() as {runs:Array<{id:string;status:string}>}
      status=summary.runs.find(run=>run.id===current.id)?.status
      return ['completed','failed','outcome_unknown','cancelled'].includes(status??'')
    },{timeout}).toBeTruthy()
  } finally {
    type Entry = { runId: string | null; sequence: number; payload: { type: string } }
    const history: Entry[] = []
    let cursor = 0
    for (;;) {
      const entries = await page.request.get(`/api/v1/studio-builder/sessions/${current.session}/entries?after=${cursor}`, { headers: { Authorization: current.authorization } })
      expect(entries.ok(), await entries.text()).toBeTruthy()
      const batch = await entries.json() as { items: Entry[]; nextCursor: number }
      history.push(...batch.items.filter(entry => entry.runId === current.id && entry.payload.type !== 'model_delta'))
      if (batch.items.length < 200) break
      expect(batch.nextCursor).toBeGreaterThan(cursor)
      cursor = batch.nextCursor
    }
    await test.info().attach(`run-${current.id}.json`,{body:JSON.stringify(history),contentType:'application/json'})
  }
  expect(status, `Run ${current.id}; see its attached events`).toBe('completed')
  await expect(panel.getByText(/创作完成，请检查|创作停止，请检查|模型调用结果未知/).last()).toBeVisible({ timeout: 180_000 })
  await expect(panel.getByText(/创作完成，请检查|创作停止，请检查|模型调用结果未知/).last()).toHaveText('创作完成，请检查静态结果并手动测试。')
  if (requireApplied) await expect(panel.getByText('已应用一个编辑事务', { exact: true }).last()).toBeVisible()
}

export async function saved(page: Page, token: string, created: Created) {
  await expect(page.locator('header').getByText(/^修订号 \d+ · 已保存$/)).toBeVisible({timeout:90_000})
  await expect(page.getByRole('button', { name: '保存', exact: true })).toBeDisabled({ timeout: 60_000 })
  return read<Draft>(page, token, `/workflows/${created.id}/drafts/${created.createdDraftId}`)
}

export const editorPath = (created: Created) => `/workflows/${created.id}/drafts/${created.createdDraftId}/editor`

export async function rename(page: Page, label: string, id = 'p8-decision', kind: 'node' | 'loop' = 'node') {
  const card = kind === 'loop' ? page.getByTestId(`studio-loop-${id}`).locator('header') : page.getByTestId(`studio-node-${id}`)
  await card.dblclick()
  const basics = page.getByTestId('node-basics')
  if (await basics.getAttribute('open') === null) await basics.locator('summary').click()
  const name=basics.getByLabel('名称', {exact: false}).first()
  await name.fill(label)
  await name.press('Tab')
  await expect(name).toHaveValue(label)
  await expect(card).toContainText(label)
  await page.getByTestId('node-details-view').getByRole('button', {name:'关闭',exact:true}).click()
}

export async function createVersion(page: Page, created: Created) {
  await page.goto(`/workflows/${created.id}`)
  await page.getByRole('combobox', {name:'选择当前草稿'}).click()
  await page.locator(`[data-option-value="${created.createdDraftId}"]`).click()
  const pending = page.waitForResponse(response => response.url().endsWith(`/workflows/${created.id}/versions`) && response.request().method() === 'POST')
  await page.getByRole('button', {name:'保存为版本',exact:true}).click()
  const response = await pending
  expect(response.status(), await response.text()).toBe(201)
  return await response.json() as {id:string;versionNumber:number;sourceDraftId:string;sourceRevision:number;sourceRevisionId:string;contentHash:string}
}

export async function derive(page: Page, created: Created, version: string, name: string): Promise<Created> {
  await page.goto(`/workflows/${created.id}`)
  await page.getByRole('button',{name:'新建草稿',exact:true}).click()
  const dialog = page.getByRole('dialog',{name:'新建草稿'})
  await dialog.getByLabel('草稿名称').fill(name)
  if (version) {
    await dialog.getByRole('combobox',{name:'基于版本'}).click()
    await page.locator(`[data-option-value="${version}"]`).click()
  }
  const pending = page.waitForResponse(response => response.url().endsWith(`/workflows/${created.id}/drafts`) && response.request().method() === 'POST')
  await dialog.getByRole('button',{name:'新建',exact:true}).click()
  const response = await pending
  expect(response.status(), await response.text()).toBe(201)
  const draft = await response.json() as Draft
  expect(draft.revision).toBe(0)
  return {id:created.id,createdDraftId:draft.id}
}

export async function publishVersion(page: Page, token: string, created: Created, version: string) {
  await page.goto(`/workflows/${created.id}`)
  await page.getByRole('button',{name:'发布到环境…',exact:true}).click()
  const dialog=page.getByRole('dialog',{name:'发布到环境…',exact:true})
  await dialog.getByRole('combobox',{name:'发布内容',exact:true}).click()
  await page.locator(`[data-option-value="${version}"]`).click()
  const pending=page.waitForResponse(response=>response.url().endsWith(`/workflows/${created.id}/deployments`)&&response.request().method()==='POST')
  await dialog.getByRole('button',{name:'发布到环境…',exact:true}).click()
  expect((await pending).ok()).toBeTruthy()
  await expect.poll(async()=> (await read<Array<{status:string;workflowVersionId:string}>>(page,token,`/workflows/${created.id}/deployments`)).find(item=>item.workflowVersionId===version)?.status,{timeout:180_000}).toBe('active')
}

export async function manualRun(page: Page, token: string, created: Created, mode='full') {
  const pending = page.waitForResponse(response => response.url().endsWith(`/drafts/${created.createdDraftId}/debug-executions`) && response.request().method()==='POST')
  await page.locator('header').getByRole('button',{name:'运行',exact:true}).click()
  const dialog=page.getByRole('dialog',{name:'调试输入',exact:true})
  if (mode === 'single_node' || mode === 'from_node') {
    await expect(dialog).toBeVisible()
    await dialog.getByRole('button',{name:'运行',exact:true}).click()
  }
  const response = await pending
  expect(response.status(),await response.text()).toBe(202)
  const value=await response.json() as {executionId:string}
  await expect.poll(async () => (await read<{status:string}>(page,token,`/executions/${value.executionId}`)).status,{timeout:120_000}).toBe('succeeded')
  return read<{id:string;source:{kind:string;draftId:string;revision:number;revisionId:string};workflowVersionId:null;executionType:string}>(page,token,`/executions/${value.executionId}`)
}

export async function fixtureCanvas(page: Page, name: string) {
  const token=await login(page)
  await setModel(page, process.env.AGENTX_E2E_STUDIO_FIXTURE_ALIAS!)
  const created=await create(page,`${name} ${Date.now()}`)
  await page.getByRole('button',{name:'AI 创作',exact:true}).click()
  await send(page,'P8FIX: label=Initial')
  await completed(page)
  await saved(page,token,created)
  return {token,created}
}
