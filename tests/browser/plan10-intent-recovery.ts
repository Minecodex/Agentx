import { expect, type Page } from '@playwright/test'
import { businessFrame, fillFields } from './plan9-page-support'
import { trackAccessToken } from './browser-http'
import { evidence } from './plan10-authoring'

type Token=ReturnType<typeof trackAccessToken>

export async function recoverLostBatchResponse(page:Page,token:Token,suffix:string,attempt:number){
  await page.goto(`/a/${suffix}`)
  let frame=await businessFrame(page)
  const values=Array.from({length:3},(_,index)=>({customer:`断线恢复-${index}-${crypto.randomUUID().slice(0,8)}`,quantity:5+index,unitPrice:37.25,discountPercent:4}))
  const fill=async()=>{
    await frame.locator('[data-action="batch-mode"]').click()
    const rows=frame.locator('[data-batch-row]')
    while(await rows.count()<3)await frame.locator('[data-action="add-batch-item"]').click()
    while(await rows.count()>3)await rows.last().locator('[data-action="remove-batch-item"]').click()
    for(let index=0;index<3;index++)await fillFields(rows.nth(index),values[index])
  }
  await fill()
  const responses:Array<{batchId:string;key:string;input:unknown}>=[],posts:string[]=[]
  const pattern=`**/gateway/v1/pages/${suffix}/batches`
  const listener=(request:import('@playwright/test').Request)=>{if(request.method()==='POST'&&request.url().endsWith(`/pages/${suffix}/batches`))posts.push(request.url())}
  page.on('request',listener)
  await page.route(pattern,async route=>{
    const response=await route.fetch()
    expect(response.status(),await response.text()).toBe(202)
    const body=await response.json() as {batchId:string}
    responses.push({batchId:body.batchId,key:route.request().headers()['idempotency-key']!,input:route.request().postDataJSON()})
    await route.abort('failed')
  })
  try{
    await frame.locator('[data-action="create-batch"]').click()
    await expect.poll(()=>responses.length,{timeout:120_000}).toBe(2)
    expect(responses[0]!.batchId).toBe(responses[1]!.batchId)
    expect(responses[0]!.key).toBe(responses[1]!.key)
    const retained=frame.locator('[data-batch-row]')
    await expect(retained).toHaveCount(3)
    for(let index=0;index<3;index++){
      await expect(retained.nth(index).locator('[data-field-path="/customer"]')).toHaveValue(values[index]!.customer)
      await expect(retained.nth(index).locator('[data-field-path="/quantity"]')).toHaveValue(String(values[index]!.quantity))
    }
    const original=responses[0]!.batchId
    evidence(`quote-response-recovery-${attempt}`,{stage:'responses_lost',responses})
    await page.unroute(pattern)
    await page.reload();frame=await businessFrame(page);await fill()
    await frame.locator('[data-action="create-batch"]').click()
    await expect.poll(async()=>{
      if(new URL(page.url()).searchParams.get('batchId')===original)return true
      // A newly generated display name can differ after reload. The host then
      // confirms the original and directs the user to their original history.
      const list=frame.locator('[data-action="list-batches"]')
      if(await list.isVisible())await list.click()
      const entry=frame.locator(`[data-batch-id="${original}"]`)
      if(await entry.isVisible())await entry.click()
      return new URL(page.url()).searchParams.get('batchId')===original
    },{timeout:60_000,intervals:[500,1000]}).toBe(true)
    await expect(page.getByTestId('application-current-batch')).toHaveText(original)
    expect(posts,'Confirmation after reload must not issue a new create request').toHaveLength(2)
    const response=await page.request.get(`${process.env.AGENTX_E2E_RUNTIME_URL}/gateway/v1/batches/${original}/items`,{headers:{Authorization:`Bearer ${await token()}`}})
    expect(response.ok(),await response.text()).toBe(true)
    const projects=(await response.json() as {items:Array<{input:unknown}>}).items
    expect(projects).toHaveLength(3);expect(projects.map(item=>item.input)).toEqual(values)
    evidence(`quote-response-recovery-${attempt}`,{passed:true,responses,projects,formalPostCount:posts.length,restoredBatchId:original})
  }finally{await page.unroute(pattern);page.off('request',listener)}
}
