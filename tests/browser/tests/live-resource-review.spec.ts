import { expect, type Page, test } from '@playwright/test'
import { writeFile } from 'node:fs/promises'

function required(name: string): string {
  const value = process.env[name]
  if (!value) throw new Error(`Missing ${name}`)
  return value
}

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('用户名').fill(user)
  await page.getByLabel('密码').fill(password)
  await page.getByRole('button', { name: '登录', exact: true }).click()
  await expect(page).toHaveURL(/\/$/)
}

async function decide(page: Page, action: '通过' | '拒绝') {
  await page.getByRole('button', { name: action, exact: true }).click()
  const confirmation = page.getByRole('dialog', { name: action, exact: true })
  await expect(confirmation).toBeVisible()
  await confirmation.getByRole('button', { name: action, exact: true }).click()
  await expect(confirmation).toBeHidden()
}

test('real resource requests display departmental redaction, co-sign approval and rejection', async ({ browser }, testInfo) => {
  test.setTimeout(180_000)
  const errors: string[] = []
  const contextOptions = { baseURL: required('AGENTX_E2E_BASE_URL'), viewport: { width: 1440, height: 900 }, locale: 'zh-CN' }
  const contexts = await Promise.all([browser.newContext(contextOptions), browser.newContext(contextOptions)])
  try {
    const [model, credential] = await Promise.all(contexts.map((context) => context.newPage()))
    for (const page of [model, credential]) page.on('pageerror', (error) => errors.push(error.message))
    await login(model, required('AGENTX_E2E_REVIEW_MODEL_USER'), required('AGENTX_E2E_REVIEW_MODEL_PASSWORD'))
    await login(credential, required('AGENTX_E2E_REVIEW_SECRET_USER'), required('AGENTX_E2E_REVIEW_SECRET_PASSWORD'))
    const requestId = required('AGENTX_E2E_REVIEW_REQUEST_ID')
    const workflowName = required('AGENTX_E2E_REVIEW_WORKFLOW_NAME')
    await model.goto('/approvals?tab=resource-grants')
    const row = model.getByRole('row').filter({ hasText: workflowName })
    await expect(row).toBeVisible()
    await expect(row.getByRole('cell').nth(5)).not.toHaveText('—')
    await row.getByRole('link', { name: '处理' }).click()
    await expect(model).toHaveURL(new RegExp(`/approvals/resource-grants/${requestId}$`))
    await expect(model.getByText(required('AGENTX_E2E_REVIEW_MODEL_NAME'), { exact: true }).first()).toBeVisible()
    await expect(model.getByText(required('AGENTX_E2E_REVIEW_SECRET_NAME'), { exact: true })).toHaveCount(0)
    await expect(model.getByText('受控依赖', { exact: true })).toBeVisible()
    await model.screenshot({ path: testInfo.outputPath('model-review-redacted.png'), fullPage: true })
    await decide(model, '通过')
    await expect(model.getByText('审批中', { exact: true }).first()).toBeVisible()
    await credential.goto(`/approvals/resource-grants/${requestId}`)
    await expect(credential.getByText(required('AGENTX_E2E_REVIEW_MODEL_NAME'), { exact: true })).toHaveCount(0)
    await expect(credential.getByText(required('AGENTX_E2E_REVIEW_SECRET_NAME'), { exact: true })).toBeVisible()
    await credential.screenshot({ path: testInfo.outputPath('credential-review-redacted.png'), fullPage: true })
    await decide(credential, '通过')
    await expect(credential.getByText('已通过', { exact: true }).first()).toBeVisible()
    await expect(credential.getByRole('button', { name: '通过', exact: true })).toHaveCount(0)
    await expect(credential.getByText('部门会签通过', { exact: true })).toHaveCount(2)
    await credential.screenshot({ path: testInfo.outputPath('co-sign-approved.png'), fullPage: true })
    await model.goto(`/approvals/resource-grants/${required('AGENTX_E2E_REVIEW_REJECT_ID')}`)
    await decide(model, '拒绝')
    await expect(model.getByText('已拒绝', { exact: true }).first()).toBeVisible()
    await model.screenshot({ path: testInfo.outputPath('resource-request-rejected.png'), fullPage: true })
    expect(errors).toEqual([])
    await writeFile(testInfo.outputPath('resource-review-results.json'), JSON.stringify({ requestId, crossDepartmentRedaction: true, allReviewsRequired: true, rejectionDisplayed: true, pageErrors: errors }, null, 2))
  } finally {
    await Promise.all(contexts.map((context) => context.close()))
  }
})
