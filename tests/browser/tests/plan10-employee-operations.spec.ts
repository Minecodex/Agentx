import { expect, test } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { businessFrame, loginAt } from '../plan9-page-support'
import { clickForResponse } from '../browser-http'
import { evidence } from '../plan10-authoring'

type Batch={batchId:string;callerId:string;callerType:string;sourceBatchId:string|null;pagePublicationId:string|null;counts:{total:number;cancelled:number}}
const input=process.env.AGENTX_E2E_PLAN10_EMPLOYEE_OPERATIONS?JSON.parse(readFileSync(process.env.AGENTX_E2E_PLAN10_EMPLOYEE_OPERATIONS,'utf8')) as {suffix:string;employeeId:string;publicationId:string;stop:Batch;cancel:Batch;failed:Batch;completed?:{batch:Batch;item:{itemId:string;input:{customer:string};task:{taskId:string}}}}:undefined

test('employee stops and cancels through the generated page SDK',async({page},info)=>{
  test.setTimeout(180_000);expect(input).toBeTruthy()
  await loginAt(page,`/a/${input!.suffix}`,process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!)
  page.on('dialog',dialog=>void dialog.accept())
  const receipts=[]
  for(const action of ['stop','cancel']as const){
    const batch=input![action]
    await page.goto(`/a/${input!.suffix}?batchId=${batch.batchId}`)
    await expect(page.getByTestId('application-current-batch')).toHaveText(batch.batchId)
    const frame=await businessFrame(page)
    const response=await clickForResponse(page,frame.locator(`[data-action="${action}-batch"]`),response=>response.url().endsWith(`/gateway/v1/batches/${batch.batchId}/${action}`)&&response.request().method()==='POST')
    expect(response.ok(),await response.text()).toBe(true)
    const result=await response.json() as Batch
    expect(result.counts.cancelled).toBe(3);expect(result.callerId).toBe(input!.employeeId)
    receipts.push({action,batch:result,url:response.url()})
    await page.screenshot({path:info.outputPath(`employee-${action}.png`),fullPage:true})
  }
  evidence('E10-U06-employee-stop-cancel',{passed:true,receipts})
})

test('employee safe rerun creates a new batch using the current publication',async({page},info)=>{
  test.setTimeout(240_000);expect(input).toBeTruthy()
  const original=input!.failed
  await loginAt(page,`/a/${input!.suffix}?batchId=${original.batchId}`,process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!)
  page.on('dialog',dialog=>void dialog.accept())
  await expect(page.getByTestId('application-current-batch')).toHaveText(original.batchId)
  const frame=await businessFrame(page)
  const response=await clickForResponse(page,frame.locator('[data-action="rerun-batch"]'),response=>response.url().endsWith(`/gateway/v1/batches/${original.batchId}/rerun`)&&response.request().method()==='POST')
  expect(response.ok(),await response.text()).toBe(true)
  const derived=await response.json() as Batch
  expect(derived.batchId).not.toBe(original.batchId)
  expect(derived.sourceBatchId).toBe(original.batchId)
  expect(derived.callerType).toBe('user');expect(derived.callerId).toBe(input!.employeeId)
  expect(derived.pagePublicationId).toBe(input!.publicationId)
  expect(derived.counts.total).toBe(3)
  await expect(page).toHaveURL(new RegExp(`batchId=${derived.batchId}`))
  await expect(page.getByTestId('application-current-batch')).toHaveText(derived.batchId)
  await expect.poll(async()=>{
    const current=await businessFrame(page)
    return current.getByText(derived.batchId,{exact:false}).isVisible().catch(()=>false)
  },{timeout:30_000}).toBe(true)
  await expect((await businessFrame(page)).locator('tr[data-batch-item-id]')).toHaveCount(3)
  await page.screenshot({path:info.outputPath('employee-derived-batch.png'),fullPage:true})
  evidence('E10-U06-employee-rerun',{passed:true,derived,url:response.url()})
})

test('employee opens an original item with the keyboard',async({page},info)=>{
  test.setTimeout(180_000);expect(input?.completed).toBeTruthy()
  const {batch,item}=input!.completed!
  await loginAt(page,`/a/${input!.suffix}?batchId=${batch.batchId}`,process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!)
  const frame=await businessFrame(page)
  const action=frame.locator(`[data-batch-item-id="${item.itemId}"]`).getByRole('button').first()
  await expect(action).toBeEnabled();await expect(action).toHaveAccessibleName(/\S/)
  expect(await action.evaluate(element=>(element as HTMLElement).tabIndex)).toBeGreaterThanOrEqual(0)
  const opened=page.waitForResponse(response=>response.url().endsWith(`/gateway/v1/tasks/${item.task.taskId}`)&&response.request().method()==='GET')
  await action.focus();await action.press('Enter')
  const response=await opened;expect(response.ok(),await response.text()).toBe(true)
  await expect(frame.locator('body')).toContainText(item.input.customer)
  await page.screenshot({path:info.outputPath('employee-keyboard-detail.png'),fullPage:true})
  evidence('E10-U04-employee-keyboard',{passed:true,itemId:item.itemId,taskId:item.task.taskId})
})

test('employee restores original generated batch pages after republishing',async({page},info)=>{
  test.setTimeout(240_000)
  type Saved={publication:{id:string;slug:string};batches:Array<{batch:{batchId:string};projects:Array<{clientItemId:string}>}>}
  const restored:unknown[]=[],writes:string[]=[]
  await loginAt(page,'/a/'+(JSON.parse(readFileSync(join(process.env.AGENTX_E2E_PLAN10_DIRECTORY!,'expense-complete.json'),'utf8')) as Saved).publication.slug,process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!)
  page.on('request',request=>{if(request.method()==='POST'&&/\/pages\/[^/]+\/(batches|invocations)$/.test(new URL(request.url()).pathname))writes.push(request.url())})
  for(const kind of ['expense','quote','file']){
    const saved=JSON.parse(readFileSync(join(process.env.AGENTX_E2E_PLAN10_DIRECTORY!,`${kind}-complete.json`),'utf8')) as Saved
    for(const [index,{batch,projects}] of saved.batches.entries()){
      const response=page.waitForResponse(response=>response.request().method()==='GET'&&response.url().includes(`/gateway/v1/pages/${saved.publication.slug}?`)&&new URL(response.url()).searchParams.get('batchId')===batch.batchId)
      await page.goto(`/a/${saved.publication.slug}?batchId=${batch.batchId}`)
      const receipt=await response;expect(receipt.ok(),await receipt.text()).toBe(true)
      const original=await receipt.json() as {historical:boolean;batchId:string;taskId:string|null;publication:{publicationId:string}}
      expect(original.historical).toBe(true);expect(original.batchId).toBe(batch.batchId);expect(original.taskId).toBeNull();expect(original.publication.publicationId).toBe(saved.publication.id)
      await expect(page.getByTestId('application-current-batch')).toHaveText(batch.batchId)
      const frame=await businessFrame(page)
      for(const item of projects)await expect(frame.getByText(item.clientItemId,{exact:true}).and(frame.locator(':visible')).first()).toBeVisible()
      await frame.getByText(projects[0]!.clientItemId,{exact:true}).and(frame.locator(':visible')).first().scrollIntoViewIfNeeded()
      await page.screenshot({path:info.outputPath(`history-${kind}-${index+1}.png`),fullPage:true})
      restored.push({kind,batchId:batch.batchId,publicationId:original.publication.publicationId,visibleItems:projects.length})
    }
  }
  expect(writes,'Restoring historical batches must not create business work').toEqual([])
  evidence('E10-V02-browser-history',{passed:true,restored,businessCreates:writes.length})
})
