import {expect} from '@playwright/test'
import {test,allEntries,completed,count,fixtureCanvas,read,saved,send,type Draft} from '../studio-builder-support'

test('P8-E2E-20: Host kill and two replicas reconcile a waiting question and settle an edit once',async({page})=>{
  const {token,created}=await fixtureCanvas(page,'P8 host recovery')
  let session=''
  page.on('request',request=>{const match=request.url().match(/\/design-agent\/sessions\/([^/]+)\/runs$/);if(match)session=match[1]!})
  await send(page,'P8FIX: host_restart question label=Recovered')
  const panel=page.getByTestId('studio-builder-panel')
  await expect(panel.getByText('Which label should the node use?',{exact:true})).toBeVisible({timeout:120_000})
  await panel.getByRole('button',{name:'Approved',exact:true}).click()
  await completed(page)
  const draft=await saved(page,token,created)
  expect(draft.definition.nodes.find(node=>node.id==='p8-decision')?.name).toBe('Recovered')
  const entries=await read<{items:Array<{payload:{type:string;status:string}}> }>(page,token,`/design-agent/sessions/${session}/entries`)
  expect(entries.items.filter(entry=>entry.payload.type==='command_acknowledged'&&entry.payload.status==='applied')).toHaveLength(2)
  await expect.poll(async()=>{try{return await count(page,token,created.id)}catch{return -1}},{timeout:60_000}).toBe(0)
})

test('P8-E2E-20: Gateway kill retains the unknown model after the frozen Run expires, without replay or canvas mutation',async({page})=>{
  test.setTimeout(780_000)
  const {token,created}=await fixtureCanvas(page,'P8 gateway recovery')
  const before=await saved(page,token,created)
  let session=''
  page.on('request',request=>{const match=request.url().match(/\/design-agent\/sessions\/([^/]+)\/runs$/);if(match)session=match[1]!})
  const run=await send(page,'P8FIX: gateway_restart slow label=MustRemainUnknown')
  await expect.poll(async()=>{
    const value=await read<{runs:Array<{id:string;status:string}>}>(page,token,`/design-agent/sessions/${session}`)
    return value.runs.find(item=>item.id===run)?.status
  // The shared design Run has a frozen ten-minute budget. Its expiry
  // cancels creation; Python separately verifies the original model Effect
  // remains outcome_unknown, has one dispatch, and expires at that deadline.
  },{timeout:660_000}).toBe('cancelled')
  const after=await read<Draft>(page,token,`/workflows/${created.id}/drafts/${created.documentId}`)
  expect(after.definition).toEqual(before.definition)
  expect(after.revision).toBe(before.revision)
  await expect.poll(async()=>{try{return await count(page,token,created.id)}catch{return -1}},{timeout:60_000}).toBe(0)
  const original=await allEntries<{runId:string|null;payload:{type:string}}>(page,token,session)
  const intents=original.filter(entry=>entry.runId===run&&entry.payload.type==='model_intent').length
  const replacement=await send(page,'P8FIX: label=ExplicitNewRun')
  expect(replacement).not.toBe(run)
  await completed(page)
  const repaired=await saved(page,token,created)
  expect(repaired.definition.nodes.find(node=>node.id==='p8-decision')?.name).toBe('ExplicitNewRun')
  const runs=await read<{runs:Array<{id:string;status:string}>}>(page,token,`/design-agent/sessions/${session}`)
  expect(runs.runs.find(item=>item.id===run)?.status).toBe('cancelled')
  const final=await allEntries<{runId:string|null;payload:{type:string}}>(page,token,session)
  expect(final.filter(entry=>entry.runId===run&&entry.payload.type==='model_intent')).toHaveLength(intents)
  expect(await count(page,token,created.id)).toBe(0)
})
