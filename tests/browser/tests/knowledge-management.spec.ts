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

async function createKnowledge(page: Page, token: string, stamp: string) {
  const departments = (await (await page.request.get('/api/v1/departments?pageSize=100', { headers: { Authorization: `Bearer ${token}` } })).json()) as unknown as Array<{ id: string }>
  const department = departments[0]
  void stamp
  // create connection + resource via API (UI form is covered by m2.1 spec)
  // The e2e LightRAG fixture enforces its fixture API key on insert/query.
  const credential = await page.request.post('/api/v1/credentials', {
    headers: { Authorization: `Bearer ${token}` },
    data: { name: `E2E Knowledge Key ${stamp}`, credentialType: 'bearer', secret: process.env.AGENTX_E2E_LIGHTRAG_API_KEY ?? 'agentx-v2-04-rag-key', ownerDepartmentId: department.id },
  })
  if (credential.status() !== 201 && credential.status() !== 200) throw new Error(`credential: ${credential.status()} ${await credential.text()}`)
  const credentialId = ((await credential.json()) as { id: string }).id
  const connection = await page.request.post('/api/v1/knowledge/connections', {
    headers: { Authorization: `Bearer ${token}` },
    data: { name: `E2E Management Connection ${stamp}`, provider: 'lightrag', endpoint: process.env.AGENTX_E2E_LIGHTRAG_BASE_URL ?? 'http://lightrag:9621', healthPath: '/health', credentialId, ownerDepartmentId: department.id, configuration: {} },
  })
  if (connection.status() !== 201 && connection.status() !== 200) throw new Error(`connection: ${connection.status()} ${await connection.text()}`)
  const connectionId = ((await connection.json()) as { id: string }).id
  const resource = await page.request.post('/api/v1/knowledge/resources', {
    headers: { Authorization: `Bearer ${token}` },
    data: { connectionId, name: `E2E Management Resource ${stamp}`, externalResourceId: `e2e_mgmt_${stamp}`, ownerDepartmentId: department.id },
  })
  if (resource.status() !== 201 && resource.status() !== 200) throw new Error(`resource: ${resource.status()} ${await resource.text()}`)
  const created = (await resource.json()) as { id: string }
  return { connectionId, resourceId: created.id, departmentId: department.id }
}

async function uploadDocument(page: Page, token: string, resourceId: string, name: string, content: string) {
  const form = new FormData()
  form.append('file', new File([content], name, { type: 'text/markdown' }))
  const response = await page.request.post(`/api/v1/knowledge/resources/${resourceId}/documents`, {
    headers: { Authorization: `Bearer ${token}` },
    timeout: 90_000,
    multipart: { file: { name, mimeType: 'text/markdown', buffer: Buffer.from(content) } },
  })
  return response
}

test.describe.serial('Knowledge management', () => {
  test('upload, index, list and hit-test a document', async ({ page }, testInfo) => {
    test.setTimeout(600_000)
    const token = await login(page)
    const stamp = `${Date.now()}`
    const { resourceId } = await createKnowledge(page, token, stamp)

    // Upload a markdown document through the API (multipart).
    const content = `# E2E Knowledge Document\n\nThe secret codename for release ${stamp} is sapphire-orbit-${stamp}.`
    const upload = await uploadDocument(page, token, resourceId, `e2e-doc-${stamp}.md`, content)
    expect(upload.status()).toBe(202)
    const uploadBody = await upload.json() as { id: string; status: string }
    expect(['uploading', 'indexing']).toContain(uploadBody.status)
    let document: { status: string; externalDocumentId: string; id: string } | undefined
    await expect.poll(async () => {
      const response = await page.request.get(`/api/v1/knowledge/resources/${resourceId}/documents`, { headers: { Authorization: `Bearer ${token}` } })
      expect(response.status()).toBe(200)
      document = (await response.json()).find((item: { id: string }) => item.id === uploadBody.id)
      if (document?.status === 'failed') throw new Error(JSON.stringify(document))
      return document?.status
    }, { timeout: 300_000 }).toBe('indexed')
    expect(document?.externalDocumentId).toBeTruthy()
    const fileSource = `agentx_${uploadBody.id.replaceAll('-', '')}.txt`

    // Detail page: documents section lists the indexed document.
    await page.goto(`/knowledge/${resourceId}`)
    await expect(page.getByRole('heading', { name: '文档管理' })).toBeVisible()
    await expect(page.getByText(`e2e-doc-${stamp}.md`)).toBeVisible()
    await expect(page.getByText('已索引')).toBeVisible()

    // Duplicate upload is rejected with a stable error code.
    const duplicate = await uploadDocument(page, token, resourceId, `e2e-doc-${stamp}.md`, content)
    expect([422, 409]).toContain(duplicate.status())

    // Hit-testing: LightRAG indexing continues in the background after the
    // documents/text ack, so poll the retrieval-test until chunks land.
    let result: { documents?: Array<Record<string, unknown>> } | undefined
    const retrievalDeadline = Date.now() + 180_000
    while (Date.now() < retrievalDeadline) {
      const retrieval = await page.request.post(`/api/v1/knowledge/resources/${resourceId}/retrieval-test`, {
        headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
        data: { query: `release codename ${stamp}`, topK: 5 },
        timeout: 30_000,
      })
      if (!retrieval.ok()) throw new Error(`retrieval: ${retrieval.status()} ${await retrieval.text()}`)
      const body = (await retrieval.json()) as { documents?: Array<Record<string, unknown>> }
      if ((body.documents?.length ?? 0) > 0) { result = body; break }
      await page.waitForTimeout(5_000)
    }
    expect(result?.documents?.length ?? 0).toBeGreaterThan(0)
    expect(JSON.stringify(result)).toContain(fileSource)

    // UI: hit-test panel renders chunk and score.
    await page.goto(`/knowledge/${resourceId}`)
    await expect(page.getByRole('heading', { name: '检索测试' })).toBeVisible()
    await page.getByPlaceholder('输入查询内容验证检索').fill(`release codename ${stamp}`)
    await page.getByRole('button', { name: '执行' }).click()
    await expect(page.getByText(fileSource)).toBeVisible({ timeout: 120_000 })
    await page.screenshot({ path: testInfo.outputPath('knowledge-hit-testing.png'), fullPage: true })
  })
})
