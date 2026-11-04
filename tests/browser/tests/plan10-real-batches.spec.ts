import { expect, test, type Page, type Browser, type Download, type TestInfo } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { author, evidence, repairPage, type BatchScenario } from '../plan10-authoring'
import { businessFrame, displayed, executeCase, fillFields, loginAt, Reviewers, waitInputAction, waitInputScope, WorkflowExecutionFailure, type Task } from '../plan9-page-support'
import { importedQuoteBatch } from '../plan10-import'
import { recoverLostBatchResponse } from '../plan10-intent-recovery'
import { desktopMatrix } from '../plan10-desktop'
import { clickForResponse, trackAccessToken } from '../browser-http'

type Batch={batchId:string;counts:{total:number;ended:number;completed:number;waitingInput:number;failed:number};[key:string]:unknown}
type Item={itemId:string;clientItemId:string;input:Record<string,unknown>;task:Task|null;state:string}
type Token=ReturnType<typeof trackAccessToken>
const scenarios:BatchScenario[]=process.env.AGENTX_E2E_PLAN10_SCENARIOS?JSON.parse(readFileSync(process.env.AGENTX_E2E_PLAN10_SCENARIOS,'utf8')):[]
const runtime=process.env.AGENTX_E2E_RUNTIME_URL!

async function query<T>(page:Page,token:Token,path:string):Promise<T>{
  const response=await page.request.get(`${runtime}/gateway/v1${path}`,{headers:{Authorization:`Bearer ${await token()}`}})
  expect(response.ok(),await response.text()).toBe(true);return response.json() as Promise<T>
}
function unseen(scenario:BatchScenario,round:number):Record<string,unknown>[] {
  return Array.from({length:3},(_,index)=>{
    const seed=scenario.inputs[(round+index)%scenario.inputs.length]!
    const marker=`${scenario.kind}-${round+1}-${index+1}-${crypto.randomUUID().slice(0,8)}`
    if(scenario.kind==='expense')return{request_id:marker,amount:79.25+round*100+index*33.5,currency:index%2?'USD':'CNY'}
    if(scenario.kind==='quote')return{customer:marker,quantity:11+round*7+index,unitPrice:42.75+index*6.25,discountPercent:round?7.5:3.25}
    return{...seed,request_id:marker,document:{__file:{name:`${marker}.txt`,mimeType:'text/plain',content:`正式批次分析数据\n编号=${marker}\n条数=${index+21}\n`}}}
  })
}

async function settleExpense(page:Page,token:Token,suffix:string,item:Item,scenario:BatchScenario,reviewers:Reviewers,index:number){
  await page.goto(`/a/${suffix}?taskId=${item.task!.taskId}`)
  await expect(page.getByTestId('application-current-task')).toHaveText(item.task!.taskId)
  let task:Task|undefined
  await expect.poll(async()=>{
    task=await query<Task>(page,token,`/tasks/${item.task!.taskId}`)
    return task.pendingInputs.length>0
  },{timeout:180_000,intervals:[500,1000]}).toBe(true)
  const wait=task!.pendingInputs[0]!,frame=await businessFrame(page),scope=waitInputScope(frame,wait.waitId)
  await expect(scope).toBeVisible()
  const note=`补充正式报销说明 ${index}`
  await fillFields(scope,{note})
  // A full batch refresh must not erase the original task's in-progress form.
  await page.waitForTimeout(2300)
  await expect(scope.locator('[data-field-path="/note"]')).toHaveValue(note)
  const sent=await clickForResponse(page,await waitInputAction(scope),response=>response.url().endsWith(`/invocations/${item.task!.taskId}/waits/${wait.waitId}/input`)&&response.request().method()==='POST')
  expect(sent.ok(),await sent.text()).toBe(true)
  await expect.poll(async()=>{
    task=await query<Task>(page,token,`/tasks/${item.task!.taskId}`);return task.pendingApprovals.length>0
  },{timeout:180_000,intervals:[500,1000]}).toBe(true)
  const decision=await reviewers.decide(task!.pendingApprovals[0]!.approvalId,{actor:'manager',decision:'approved'},index,{...scenario,scene:'B10'})
  await expect.poll(async()=>{task=await query<Task>(page,token,`/tasks/${item.task!.taskId}`);return task.state},{timeout:180_000,intervals:[500,1000]}).toBe('completed')
  expect(task!.outputs).toMatchObject({...item.input,notes:{note},decision:'approved'})
  return{waitId:wait.waitId,note,approval:decision,final:task}
}

async function business(browser:Browser,scenario:BatchScenario,suffix:string,info:TestInfo,attempt:number){
  const context=await browser.newContext({baseURL:process.env.AGENTX_E2E_BASE_URL,viewport:{width:1440,height:900},locale:'zh-CN'})
  const page=await context.newPage(),reviewers=new Reviewers(browser,info)
  const result:unknown[]=[]
  const acceptedBatches:Batch[]=[]
  const errors:string[]=[];page.on('pageerror',error=>errors.push(error.message))
  const token=trackAccessToken(page,await loginAt(page,`/a/${suffix}`,process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!))
  try{
    await desktopMatrix(browser,scenario,suffix,info,attempt)
    for(let round=0;round<2;round++){
      await page.goto(`/a/${suffix}`)
      let frame=await businessFrame(page)
      const submissions:Array<{url:string;body:unknown}>=[]
      const listener=(request:import('@playwright/test').Request)=>{
        if(request.method()==='POST'&&(/\/pages\/[^/]+\/(batches|invocations)$/.test(new URL(request.url()).pathname)))submissions.push({url:request.url(),body:request.postDataJSON()})
      }
      page.on('request',listener)
      await frame.locator('[data-action="batch-mode"]').click()
      const rows=frame.locator('[data-batch-row]'),input=unseen(scenario,round)
      while(await rows.count()<3)await frame.locator('[data-action="add-batch-item"]').click()
      while(await rows.count()>3)await rows.last().locator('[data-action="remove-batch-item"]').click()
      const previousIds=await rows.evaluateAll(elements=>elements.map(element=>element.getAttribute('data-client-item-id')))
      await rows.nth(1).locator('[data-action="remove-batch-item"]').click()
      await expect(rows).toHaveCount(2)
      expect(await rows.evaluateAll(elements=>elements.map(element=>element.getAttribute('data-client-item-id')))).toEqual([previousIds[0],previousIds[2]])
      await frame.locator('[data-action="add-batch-item"]').click()
      await expect(rows).toHaveCount(3)
      for(let index=0;index<3;index++)await fillFields(rows.nth(index),input[index])
      const rowIds=await rows.evaluateAll(elements=>elements.map(element=>element.getAttribute('data-client-item-id')))
      expect(new Set(rowIds).size).toBe(3)
      expect(rowIds.slice(0,2)).toEqual([previousIds[0],previousIds[2]])
      expect(submissions,'Mode switches, row edits and uploads cannot submit business work').toEqual([])
      const accepted=await clickForResponse(page,frame.locator('[data-action="create-batch"]'),response=>response.url().endsWith(`/pages/${suffix}/batches`)&&response.request().method()==='POST')
      expect(accepted.status(),await accepted.text()).toBe(202)
      const batch=await accepted.json() as Batch
      acceptedBatches.push(batch)
      evidence(`${scenario.kind}-business-${attempt}`,{acceptedBatches,batches:result,stage:'accepted'})
      expect(batch.counts.total).toBe(3)
      await expect(page.getByTestId('application-current-batch')).toHaveText(batch.batchId)
      await expect(page).toHaveURL(new RegExp(`batchId=${batch.batchId}`))
      // Close the frame and reconstruct it through its public URL before waiting for work.
      await page.reload();await expect(page.getByTestId('application-current-batch')).toHaveText(batch.batchId)
      let projects:Item[]=[]
      await expect.poll(async()=>{
        projects=(await query<{items:Item[]}>(page,token,`/batches/${batch.batchId}/items`)).items
        return projects.length===3&&projects.every(item=>item.task!==null)
      },{timeout:180_000,intervals:[500,1000]}).toBe(true)
      expect(projects.map(item=>item.clientItemId)).toEqual(rowIds)
      if(scenario.kind==='expense'){
        await expect.poll(async()=>(await query<Batch>(page,token,`/batches/${batch.batchId}`)).counts.waitingInput,{timeout:180_000,intervals:[500,1000]}).toBe(3)
        const waitingFrame=await businessFrame(page)
        await expect(waitingFrame.locator('body'),'The displayed pending-input count must match all three real waiting items').toContainText(/(?:待补充(?:资料|输入)?|等待(?:补充(?:资料|输入)?|资料|输入))\s*[:：]?\s*3\b/,{timeout:30_000})
      }
      for(let index=0;index<3;index++){
        const actual=projects[index]!.input,expected=input[index]!
        if(scenario.kind==='file'){
          const file=(expected.document as {__file:{content:string}}).__file
          expect(actual.document).toMatchObject({contentType:'text/plain',sizeBytes:Buffer.byteLength(file.content),sha256:createHash('sha256').update(file.content).digest('hex')})
        }else expect(actual).toEqual(expected)
        expect(projects[index]!.task!.sessionId).toBeNull()
      }
      const interactions=[]
      if(scenario.kind==='expense')for(let index=0;index<3;index++)interactions.push(await settleExpense(page,token,suffix,projects[index]!,scenario,reviewers,round*3+index))
      let settled:Batch|undefined
      await expect.poll(async()=>{settled=await query<Batch>(page,token,`/batches/${batch.batchId}`);return settled.counts.ended},{timeout:360_000,intervals:[1000,2000]}).toBe(3)
      projects=(await query<{items:Item[]}>(page,token,`/batches/${batch.batchId}/items`)).items
      const failed=projects.find(item=>item.task&&['failed','outcome_unknown'].includes(item.task.state))
      if(failed)throw new WorkflowExecutionFailure(failed.task!,'A formal workflow failed; page repairs cannot change the frozen execution')
      expect(settled!.counts.completed,JSON.stringify(projects)).toBe(3)
      for(const item of projects){
        if(scenario.kind==='quote'){
          const value=item.input as {customer:string;quantity:number;unitPrice:number;discountPercent:number}
          expect(item.task!.outputs).toMatchObject({customer:value.customer,currency:'CNY'})
          expect(item.task!.outputs.total).toBeCloseTo(value.quantity*value.unitPrice*(1-value.discountPercent/100),2)
        }
        if(scenario.kind==='file')expect(item.task!.outputs).toMatchObject({request_id:item.input.request_id,...Object.fromEntries([['size_bytes',(item.input.document as {sizeBytes:number}).sizeBytes],['sha256',(item.input.document as {sha256:string}).sha256]])})
        if(scenario.kind!=='expense'){
          await page.goto(`/a/${suffix}?taskId=${item.task!.taskId}`)
          await expect(page.getByTestId('application-current-task')).toHaveText(item.task!.taskId)
          await displayed(await businessFrame(page),Object.fromEntries(Object.entries(item.task!.outputs).filter(([key])=>key!=='files')))
        }
      }
      await page.goto(`/a/${suffix}?batchId=${batch.batchId}`);frame=await businessFrame(page)
      await expect(frame.locator('body')).toContainText(/完成|成功|completed/i,{timeout:30_000})
      for(const item of projects)await expect(frame.getByText(item.clientItemId,{exact:true}).and(frame.locator(':visible')).first(),'All original batch rows must remain visible after the final poll').toBeVisible()
      const downloads:Download[]=[]
      const downloading=(download:Download)=>downloads.push(download)
      page.on('download',downloading)
      const exporting=page.waitForResponse(response=>response.url().endsWith(`/batches/${batch.batchId}/exports`)&&response.request().method()==='POST')
      await frame.locator('[data-action="export-batch"]:visible').first().click()
      const exportReceipt=await exporting;expect(exportReceipt.ok(),await exportReceipt.text()).toBe(true)
      const exportId=(await exportReceipt.json() as {exportId:string}).exportId
      let exported:{status:string;file:{artifactId:string}}|undefined
      await expect.poll(async()=>{exported=await query(page,token,`/batches/${batch.batchId}/exports/${exportId}`);return exported!.status},{timeout:60_000}).toBe('completed')
      const csv=await page.request.get(`${runtime}/gateway/v1/batches/${batch.batchId}/exports/${exportId}/file`,{headers:{Authorization:`Bearer ${await token()}`}})
      expect(csv.ok()).toBe(true);expect(await csv.text()).toContain('completed')
      if(!downloads.length)await frame.locator('[data-action="download-batch-export"]:visible').first().click()
      await expect.poll(()=>downloads.length>0,{timeout:30_000}).toBe(true)
      const savedCsv=info.outputPath(`${scenario.kind}-batch-${round+1}.csv`)
      expect(downloads[0]!.suggestedFilename()).toBe(`batch-${batch.batchId}.csv`)
      await downloads[0]!.saveAs(savedCsv)
      expect(readFileSync(savedCsv,'utf8')).toBe(await csv.text())
      page.off('download',downloading)
      page.off('request',listener)
      expect(submissions.filter(request=>request.url.endsWith('/batches'))).toHaveLength(1)
      expect(submissions.filter(request=>request.url.endsWith('/invocations'))).toHaveLength(0)
      for(const item of projects)await expect(frame.getByText(item.clientItemId,{exact:true}).and(frame.locator(':visible')).first(),'Export refresh must preserve the original batch rows').toBeVisible()
      expect(await frame.locator('body').innerText(),'Business status labels and row identifiers must use readable UI labels instead of SDK property names').not.toMatch(/waiting_input|waiting_approval|outcome_unknown|clientItemId|c_accepted|c_finished/)
      if(await page.locator('html').getAttribute('lang')==='zh-CN')await expect(frame.getByText('completed',{exact:true}).and(frame.locator(':visible')),'Chinese status badges must use localized labels').toHaveCount(0)
      await page.screenshot({path:info.outputPath(`${scenario.kind}-batch-${round+1}.png`),fullPage:true})
      result.push({batch,inputs:input,projects,interactions,settled,export:exported,csv:await csv.text(),submissions})
      evidence(`${scenario.kind}-business`,{batches:result,errors})
      evidence(`${scenario.kind}-business-${attempt}`,{batches:result,acceptedBatches,errors})
    }
    if(scenario.kind==='quote'){
      const imported=await importedQuoteBatch(page,token,suffix,attempt)
      evidence('quote-import',imported)
      await recoverLostBatchResponse(page,token,suffix,attempt)
    }
    expect(errors).toEqual([])
    return result
  }catch(error){
    evidence(`${scenario.kind}-business-${attempt}`,{passed:false,error:error instanceof Error?error.message:String(error),batches:result,acceptedBatches,errors})
    for(const batch of acceptedBatches){
      const current=await query<Batch>(page,token,`/batches/${batch.batchId}`)
      if(current.canCancel){
        const response=await page.request.post(`${runtime}/gateway/v1/batches/${batch.batchId}/cancel`,{headers:{Authorization:`Bearer ${await token()}`,'Idempotency-Key':`validation-cleanup:${batch.batchId}`}})
        expect(response.ok(),await response.text()).toBe(true)
      }
    }
    throw error
  }finally{await reviewers.close();await context.close()}
}

for(const scenario of scenarios)test(`${scenario.kind}: real AI page, two unseen durable batches and original tasks`,async({page,browser},info)=>{
  test.setTimeout(2_100_000)
  let publication=await author(page,scenario)
  for(let attempt=0;attempt<3;attempt++){
    try{
      const batches=await business(browser,scenario,publication.suffix,info,attempt)
      const single=scenario.kind==='expense'?undefined:await executeCase(browser,scenario,publication.suffix,0,info,attempt)
      evidence(`${scenario.kind}-complete`,{passed:true,publication:{id:publication.publicationId,slug:publication.suffix,htmlHash:publication.draft.htmlHash},batches,single})
      return
    }catch(error){
      if(error instanceof WorkflowExecutionFailure||String(error).includes('FORMAL_BUSINESS_FAILURE')||attempt===2)throw error
      publication=await repairPage(page,scenario,publication,error,attempt+1)
    }
  }
})
