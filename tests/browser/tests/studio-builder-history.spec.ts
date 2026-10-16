import { expect } from '@playwright/test'
import { test, allEntries, completed, count, derive, editorPath, fixtureCanvas, read, saved, send } from '../studio-builder-support'

test('P8-E2E-26: removed resources remain protected by immutable revisions and archived independent drafts', async ({page}) => {
  const {token, created} = await fixtureCanvas(page, 'P8 historical references')
  const model = process.env.AGENTX_E2E_STUDIO_ALTERNATE_ID!
  let session = ''
  page.on('request', request => { const match = request.url().match(/\/studio-builder\/sessions\/([^/]+)\/runs$/); if (match) session = match[1]! })
  await send(page, `P8FIX: resource_queries resource_ref=${model} label=HistoricalModel`)
  await completed(page)
  const entries = await allEntries<{payload:{type:string;tool_name?:string;message?:{role:string;content:string}}}>(page,token,session)
  expect(entries.filter(entry => entry.payload.type === 'tool_intent' && entry.payload.tool_name === 'resource_query')).toHaveLength(3)
  const metadata = entries.flatMap(entry => {
    if (entry.payload.message?.role !== 'tool_result') return []
    try { const value = JSON.parse(entry.payload.message.content); return value.metadata?.id === model ? [value] : [] } catch { return [] }
  })
  expect(metadata).toHaveLength(2)
  expect(metadata.every(value => value.optionsSource === 'platform_metadata' && value.providerInvoked === false)).toBeTruthy()
  const referenced = await saved(page, token, created)
  await send(page, 'P8FIX: clear_resource label=RemovedFromHead')
  await completed(page)
  const current = await saved(page, token, created)
  expect(current.revision).toBeGreaterThan(referenced.revision)
  const other = await derive(page, created, '', 'Independent blank')
  await page.goto(`/workflows/${created.id}`)
  const row = page.getByText('Independent blank', {exact:true}).locator('..').locator('..')
  await row.getByRole('button', {name:'归档草稿',exact:true}).click()
  const impact = await read<{references:Array<{sourceType:string;immutable:boolean;sourceName:string}>}>(page,token,`/deletion-impact/model/${model}`)
  expect(impact.references.some(item=>item.sourceType==='workflow_draft_revision'&&item.immutable&&item.sourceName.includes(`r${referenced.revision}`))).toBeTruthy()
  const blocked = await page.request.delete(`/api/v1/models/aliases/${model}`,{headers:{Authorization:`Bearer ${token}`}})
  expect(blocked.status(),await blocked.text()).toBe(409)
  expect((await blocked.json()).code).toBe('RESOURCE_REFERENCED')
  await page.goto(editorPath(created))
  expect(await count(page,token,created.id)).toBe(0)
  expect((await read<{revision:number}>(page,token,`/workflows/${other.id}/drafts/${other.createdDraftId}`)).revision).toBe(0)
})
