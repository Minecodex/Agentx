import { expect } from '@playwright/test'
import { test, completed, count, createVersion, editorPath, fixtureCanvas, read, rename, saved, send, type Draft } from '../studio-builder-support'

test('P8-E2E-04/05/06/07/26: published drafts restore their own version and invalidate AI bindings',async({page})=>{
  const {token,created}=await fixtureCanvas(page,'P8 versions')
  const initial=await saved(page,token,created)
  const v1=await createVersion(page,created)
  expect(v1.id).toBe(created.createdVersionId)
  expect(v1.sourceRevision).toBe(initial.revision)
  await page.goto(editorPath(created))
  await page.getByRole('button',{name:'AI 创作',exact:true}).click()
  await send(page,'P8FIX: label=VersionTwo')
  await completed(page)
  const second=await saved(page,token,created)
  const v2=await createVersion(page,created)
  expect(v2.versionNumber).toBe(2)
  expect((await read<{hasDraft:boolean}>(page,token,`/workflows/${created.id}/versions/${v1.id}`)).hasDraft).toBe(false)
  await page.goto(`/workflows/${created.id}/versions/${v2.id}/editor`)
  await expect(page.getByTestId('studio-node-p8-decision')).toContainText('VersionTwo')
  await page.getByRole('button',{name:'AI 创作',exact:true}).click()
  let sessionId=''
  page.on('request',request=>{const match=request.url().match(/sessions\/([^/]+)\/runs$/);if(match)sessionId=match[1]!})
  await send(page,'P8FIX: question label=PendingRestore')
  await expect(page.getByTestId('studio-builder-panel').getByLabel('回答设计问题')).toBeVisible()
  const beforeRestore=await read<{snapshot:unknown}>(page,token,`/studio-builder/sessions/${sessionId}`)
  await page.getByRole('button',{name:'还原',exact:true}).click()
  const confirm=page.getByRole('dialog',{name:'还原画布'})
  await expect(confirm).toContainText('v2')
  await confirm.getByRole('button',{name:'确认还原',exact:true}).click()
  await expect(confirm).not.toBeVisible()
  await expect(page.getByTestId('studio-node-p8-decision')).toContainText('VersionTwo')
  const contextResponse=await page.request.post(`/api/v1/workflows/${created.id}/versions/${v2.id}/edit`,{headers:{Authorization:`Bearer ${token}`}})
  const document=(await contextResponse.json()).document as Draft
  expect(document.definition).toEqual(second.definition)
  const session=await read<{runs:Array<{status:string}>}>(page,token,`/studio-builder/sessions/${sessionId}`)
  expect(session.runs[0].status).toBe('cancelled')
  const staleSnapshot=await page.request.put(`/api/v1/studio-builder/sessions/${sessionId}/snapshot`,{headers:{Authorization:`Bearer ${token}`},data:beforeRestore.snapshot})
  expect(staleSnapshot.status(),await staleSnapshot.text()).toBe(409)
  const lateCommand=await page.request.post(`/api/v1/studio-builder/sessions/${sessionId}/acknowledgements`,{headers:{Authorization:`Bearer ${token}`},data:{commandId:crypto.randomUUID(),status:'applied',snapshot:beforeRestore.snapshot,issues:[]}})
  expect(lateCommand.status(),await lateCommand.text()).toBe(409)
  expect((await read<Draft>(page,token,`/workflows/${created.id}/drafts/${document.id}`)).definition).toEqual(second.definition)
})

test('P8-E2E-08/10: saving an older snapshot keeps later manual edits; same draft conflict is resolved by UI',async({page,context})=>{
  const {token,created}=await fixtureCanvas(page,'P8 save race')
  const path=`**/workflows/${created.id}/drafts/${created.documentId}`
  let release!:()=>void
  let held!:()=>void
  const pending=new Promise<void>(resolve=>{held=resolve})
  const gate=new Promise<void>(resolve=>{release=resolve})
  let intercepted=false
  await page.route(path,async route=>{
    if(route.request().method()!=='PUT'||intercepted){await route.continue();return}
    intercepted=true
    const response=await route.fetch()
    held()
    await gate
    await route.fulfill({response})
  })
  await rename(page,'Submitted')
  await pending
  await rename(page,'LaterHumanEdit')
  release()
  await expect(page.getByTestId('studio-node-p8-decision')).toContainText('LaterHumanEdit')
  await saved(page,token,created)
  await page.unroute(path)
  await page.reload()
  await expect(page.getByTestId('studio-node-p8-decision')).toContainText('LaterHumanEdit')
  const other=await context.newPage()
  await other.goto(editorPath(created))
  await expect(other.getByTestId('studio-node-p8-decision')).toBeVisible()
  await rename(page,'WindowOne')
  await saved(page,token,created)
  await rename(other,'WindowTwo')
  const conflict=other.getByRole('dialog',{name:'草稿版本冲突'})
  await expect(conflict).toBeVisible()
  await conflict.getByRole('button',{name:'载入服务器版本',exact:true}).click()
  await expect(other.getByTestId('studio-node-p8-decision')).toContainText('WindowOne')
  expect(await count(page,token,created.id)).toBe(0)
  await other.close()
})

test('P8-E2E-09: a command prepared before a human edit is stale, re-read and replaced without overwriting',async({page})=>{
  const {token,created}=await fixtureCanvas(page,'P8 stale')
  let session=''
  page.on('request',request=>{const match=request.url().match(/\/studio-builder\/sessions\/([^/]+)\/runs$/);if(match)session=match[1]!})
  await page.route('**/studio-builder/sessions/*/events?*',route=>route.abort('connectionreset'))
  await page.reload()
  await page.getByRole('button',{name:'AI 创作',exact:true}).click()
  const run=await send(page,'P8FIX: label=AIUpdated')
  await expect.poll(async()=>{
    const entries=await read<{items:Array<{runId:string;payload:{type:string;command?:unknown}}> }>(page,token,`/studio-builder/sessions/${session}/entries`)
    return entries.items.some(entry=>entry.runId===run&&entry.payload.type==='command')
  },{timeout:60_000}).toBeTruthy()
  await rename(page,'HumanChanged')
  await saved(page,token,created)
  await page.unroute('**/studio-builder/sessions/*/events?*')
  await completed(page)
  const entries=await read<{items:Array<{payload:{type:string;status?:string}}> }>(page,token,`/studio-builder/sessions/${session}/entries`)
  expect(entries.items.some(entry=>entry.payload.type==='command_acknowledged'&&entry.payload.status==='stale')).toBeTruthy()
  expect(await count(page,token,created.id)).toBe(0)
})
