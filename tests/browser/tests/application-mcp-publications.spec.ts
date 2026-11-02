import { test, expect } from '@playwright/test'
import { writeFileSync } from 'node:fs'
import { login } from '../studio-builder-support'
import { useRuntimePortForward } from './playground-helpers'

const input = JSON.parse(process.env.AGENTX_E2E_MCP_PUBLICATIONS!) as {
  applicationId: string; phase: 'first' | 'change' | 'delete' | 'empty' | 'register' | 'authorize'
  toolName?: string; slug?: string; description?: string; revision?: number; callback?: string; authorizationUrl?: string; report: string
}

test(`MCP publication ${input.phase} uses the actual application UI`, async ({ page }, info) => {
  await useRuntimePortForward(page)
  await login(page)
  await page.goto(`/applications/${input.applicationId}?tab=mcp`)
  const tab = page.getByTestId('application-mcp-tab')
  await expect(tab).toBeVisible()
  if (input.phase === 'register') {
    await tab.locator('summary').getByText('个人 OAuth 客户端', { exact: true }).click()
    await tab.getByRole('button', { name: '登记客户端', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: '个人 OAuth 客户端', exact: true })
    await dialog.getByLabel('客户端名称').fill('Publication history OAuth')
    await dialog.getByLabel('回调地址，每行一个').fill(input.callback!)
    const response = page.waitForResponse(value => new URL(value.url()).pathname.endsWith('/mcp/clients') && value.request().method() === 'POST')
    await dialog.getByRole('button', { name: '登记客户端', exact: true }).click()
    const registered = await response
    expect(registered.ok(), await registered.text()).toBe(true)
    writeFileSync(input.report, JSON.stringify({ passed: true, client: await registered.json() }, null, 2))
  } else if (input.phase === 'authorize') {
    await page.goto(input.authorizationUrl!)
    await expect(page.getByRole('heading', { name: '连接 Agentx 账号', exact: true })).toBeVisible()
    await page.getByRole('button', { name: '继续登录', exact: true }).click()
    await expect(page.getByRole('heading', { name: '允许客户端使用应用', exact: true })).toBeVisible()
    await page.screenshot({ path: info.outputPath('mcp-consent.jpg'), type: 'jpeg', fullPage: true })
    await page.getByRole('button', { name: '允许连接', exact: true }).click()
    await expect(page).toHaveURL(url => url.origin + url.pathname === input.callback)
    writeFileSync(input.report, JSON.stringify({ passed: true }, null, 2))
  } else if (input.phase === 'first' || input.phase === 'change') {
    if (input.phase === 'first') {
      await expect(tab.getByTestId('mcp-documentation')).toHaveCount(0)
      await expect(tab.getByRole('button', { name: '发布接入', exact: true })).toBeVisible()
      await expect(tab.getByRole('textbox')).toHaveCount(0)
    }
    const label = input.phase === 'first' ? '发布接入' : '更改发布'
    await tab.getByRole('button', { name: label, exact: true }).click()
    const dialog = page.getByRole('dialog', { name: label, exact: true })
    await dialog.getByLabel('应用工具名称').fill(input.toolName!)
    await dialog.getByLabel('访问后缀').fill(input.slug!)
    if (input.phase === 'first') await expect(dialog.getByRole('textbox')).toHaveCount(2)
    else await dialog.getByLabel('用途说明').fill(input.description!)
    const response = page.waitForResponse(value => new URL(value.url()).pathname.endsWith('/mcp/publications') && value.request().method() === 'POST')
    await dialog.getByRole('button', { name: '发布接入', exact: true }).click()
    const published = await response
    expect(published.ok(), await published.text()).toBe(true)
    const revision = await published.json()
    await expect(dialog).toHaveCount(0)
    await expect(tab.getByTestId('mcp-documentation').getByRole('heading', { name: input.toolName, exact: true })).toBeVisible()
    await expect(tab.getByRole('button', { name: '更改发布', exact: true })).toBeEnabled()
    await expect(tab.getByRole('textbox')).toHaveCount(0)
    await page.screenshot({ path: info.outputPath('mcp-documentation.jpg'), type: 'jpeg', fullPage: true })
    await tab.getByRole('button', { name: '发布历史', exact: true }).click()
    const history = page.getByRole('dialog', { name: '发布历史', exact: true })
    await expect(history.getByTestId(`mcp-publication-${revision.publication.revision}`)).toBeVisible()
    await page.screenshot({ path: info.outputPath('mcp-history.jpg'), type: 'jpeg', fullPage: true })
    writeFileSync(input.report, JSON.stringify({ passed: true, revision }, null, 2))
  } else if (input.phase === 'delete') {
    await tab.getByRole('button', { name: '发布历史', exact: true }).click()
    const history = page.getByRole('dialog', { name: '发布历史', exact: true })
    await history.getByRole('button', { name: `删除发布 #${input.revision}`, exact: true }).click()
    const confirmation = page.getByRole('dialog', { name: '删除发布', exact: true })
    const response = page.waitForResponse(value => /\/mcp\/publications\/[^/]+$/.test(new URL(value.url()).pathname) && value.request().method() === 'DELETE')
    await confirmation.getByRole('button', { name: '删除', exact: true }).click()
    const deleted = await response
    expect(deleted.ok(), await deleted.text()).toBe(true)
    await expect(confirmation).toHaveCount(0)
    await expect(history.getByTestId(`mcp-publication-${input.revision}`)).toHaveCount(0)
    await page.screenshot({ path: info.outputPath('mcp-history-deleted.jpg'), type: 'jpeg', fullPage: true })
    writeFileSync(input.report, JSON.stringify({ passed: true, deleted: await deleted.json() }, null, 2))
  } else {
    await expect(tab.getByRole('button', { name: '发布接入', exact: true })).toBeEnabled()
    await expect(tab.getByTestId('mcp-documentation')).toHaveCount(0)
    await expect(tab.getByRole('button', { name: '发布历史', exact: true })).toHaveCount(0)
    await page.screenshot({ path: info.outputPath('mcp-unpublished.jpg'), type: 'jpeg', fullPage: true })
    writeFileSync(input.report, JSON.stringify({ passed: true }, null, 2))
  }
})
