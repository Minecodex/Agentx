import { expect } from '@playwright/test'
import { test, create, login, read, saved } from '../studio-builder-support'

test('P8-E2E-26: removing a plugin node keeps the package for historical revisions, and deletion releases it', async ({page}) => {
  const token = await login(page)
  const plugins = await read<{items:Array<{id:string;packageId:string;versions:Array<{id:string}>}>}>(page,token,'/canvas-plugins?pageSize=100')
  const plugin = plugins.items.find(item=>item.packageId==='acme/resolve-pure')!
  expect(plugin).toBeTruthy()
  const created = await create(page, `P8 package history ${Date.now()}`)
  await page.getByTestId('node-creator').getByRole('textbox',{name:'搜索节点'}).fill('pure')
  await page.getByTestId('palette-action-acme.resolve_pure').click()
  await expect(page.locator('.react-flow__node-manifest').filter({hasText:'pure'})).toBeVisible()
  const withNode = await saved(page,token,created)
  const node = withNode.definition.nodes.find(item=>item.type==='acme.resolve_pure')!
  expect(node).toBeTruthy()
  await page.getByTestId(`studio-node-${node.id}`).click()
  await page.keyboard.press('Delete')
  const head = await saved(page,token,created)
  expect(head.definition.nodes.some(item=>item.type==='acme.resolve_pure')).toBeFalsy()
  const references = await read<{drafts:number}>(page,token,`/canvas-plugins/${plugin.id}/references`)
  expect(references.drafts).toBe(1)
  const blocked = await page.request.delete(`/api/v1/canvas-plugins/${plugin.id}/versions/${plugin.versions[0]!.id}`,{headers:{Authorization:`Bearer ${token}`}})
  expect(blocked.status(),await blocked.text()).toBe(409)
  const removed = await page.request.delete(`/api/v1/workflows/${created.id}`,{headers:{Authorization:`Bearer ${token}`}})
  expect(removed.status(),await removed.text()).toBe(204)
  const released = await page.request.delete(`/api/v1/canvas-plugins/${plugin.id}/versions/${plugin.versions[0]!.id}`,{headers:{Authorization:`Bearer ${token}`}})
  expect(released.status(),await released.text()).toBe(204)
})
