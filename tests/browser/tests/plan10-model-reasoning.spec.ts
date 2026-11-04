import { expect, test } from '@playwright/test'
import { clickForResponse } from '../browser-http'
import { evidence } from '../plan10-authoring'
import { loginAt } from '../plan9-page-support'

const modelId = process.env.AGENTX_E2E_PLAN10_MODEL_ID

test('model reasoning selection persists max across desktop languages and themes', async ({ page, browser }, info) => {
  test.setTimeout(240_000)
  expect(modelId).toBeTruthy()
  await loginAt(page, `/models/${modelId}`, 'admin', 'agentx-e2e-admin-password')
  const edit = page.getByRole('button', { name: '编辑模型', exact: true })
  await expect(edit).toBeEnabled()
  await edit.click()
  const dialog = page.getByRole('dialog', { name: '编辑模型', exact: true })
  const json = dialog.getByLabel('默认参数 JSON')
  const original = JSON.parse(await json.inputValue()) as Record<string, unknown>
  await dialog.getByRole('combobox', { name: '思考深度', exact: true }).click()
  await page.getByRole('option', { name: '最大（max）', exact: true }).click()
  expect(JSON.parse(await json.inputValue())).toEqual({ ...original, reasoning_effort: 'max' })
  const saved = await clickForResponse(page, dialog.getByRole('button', { name: '保存', exact: true }), response => response.url().endsWith(`/models/aliases/${modelId}`) && response.request().method() === 'PATCH')
  expect(saved.ok(), await saved.text()).toBe(true)
  const model = await saved.json() as { defaultParameters: Record<string, unknown>; deploymentId: string; aliasVersion: number }
  expect(model.defaultParameters).toEqual({ ...original, reasoning_effort: 'max' })
  await expect(dialog).toHaveCount(0)
  const profiles = []
  for (const locale of ['zh-CN', 'en-US']) for (const theme of ['light', 'dark'] as const) {
    const context = await browser.newContext({ baseURL: process.env.AGENTX_E2E_BASE_URL, viewport: { width: 1440, height: 900 }, locale, colorScheme: theme })
    await context.addInitScript(({ locale, theme }) => {
      if (window !== window.top) return
      localStorage.setItem('agentx.locale', locale)
      localStorage.setItem('agentx.theme', theme)
    }, { locale, theme })
    try {
      const current = await context.newPage()
      await loginAt(current, `/models/${modelId}`, 'admin', 'agentx-e2e-admin-password')
      await expect(current.locator('html')).toHaveAttribute('lang', locale)
      await expect(current.locator('html')).toHaveAttribute('data-theme', theme)
      const label = locale === 'en-US' ? 'Edit model' : '编辑模型'
      const button = current.getByRole('button', { name: label, exact: true })
      await expect(button).toBeEnabled()
      await button.click()
      const editing = current.getByRole('dialog', { name: label, exact: true })
      await expect(editing.getByRole('combobox', { name: locale === 'en-US' ? 'Reasoning effort' : '思考深度', exact: true })).toHaveText(/max/)
      expect(JSON.parse(await editing.getByLabel(locale === 'en-US' ? 'Default parameters JSON' : '默认参数 JSON').inputValue())).toEqual(model.defaultParameters)
      expect(await current.evaluate(() => document.documentElement.scrollWidth > window.innerWidth)).toBe(false)
      await current.screenshot({ path: info.outputPath(`model-reasoning-${locale}-${theme}.png`), fullPage: true })
      profiles.push({ locale, theme, effort: 'max' })
    } finally { await context.close() }
  }
  evidence('model-reasoning-desktop', { passed: true, profiles, deploymentId: model.deploymentId, aliasVersion: model.aliasVersion })
})
