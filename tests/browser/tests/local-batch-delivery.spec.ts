import { expect, test } from '@playwright/test'
import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { businessFrame, fillFields, type Task } from '../plan9-page-support'

const configFile=process.env.AGENTX_LOCAL_BATCH_DELIVERY_CONFIG
test.skip(!configFile||process.env.AGENTX_LOCAL_BATCH_BROWSER_AUTHORIZED!=='1','Requires explicit retained site access authorization')
test.use({trace:'off',video:'off'})
type Case={applicationId:string;slug:string;draftId:string;htmlHash:string;contractHash:string;publicationId?:string}
type Batch={batchId:string;name:string;state:string;counts:{total:number;completed:number;waitingInput:number;waitingApproval:number;ended:number}}
type Item={itemId:string;clientItemId:string;ordinal:number;input:Record<string,unknown>;task:Task|null;state:string}

test('Publish the retained expense batch page and exercise two real three-item batches',async({page,context},info)=>{
  const config=JSON.parse(readFileSync(configFile!,'utf8')) as {baseUrl:string;runtimeUrl:string;evidenceDir:string;credentialsFile:string}
  const file=(name:string)=>join(config.evidenceDir,name)
  const scenario=JSON.parse(readFileSync(file('batch-scenario.json'),'utf8')) as Case
  const credentials=JSON.parse(readFileSync(config.credentialsFile,'utf8')) as {username:string;password:string}
  const errors:string[]=[]
  page.on('pageerror',error=>errors.push(error.message))
  // API transport trusts the explicit deployment CA, preserving HTTPS and
  // certificate identity checks without changing the developer's trust store.
  await context.route(/^https:\/\/(?:run\.)?agentx\.localhost\//,async route=>{
    const response=await route.fetch();await route.fulfill({response})
  })
  await page.goto(config.baseUrl+'/login')
  await page.getByLabel('用户名',{exact:true}).fill(credentials.username)
  await page.getByLabel('密码',{exact:true}).fill(credentials.password)
  const logging=page.waitForResponse(response=>response.url().endsWith('/api/v1/auth/login')&&response.request().method()==='POST')
  await page.getByRole('button',{name:'登录',exact:true}).click()
  const logged=await logging;expect(logged.ok()).toBe(true)
  const token=(await logged.json()).accessToken as string
  const headers={Authorization:'Bearer '+token}
  async function get<T>(path:string):Promise<T>{const response=await page.request.get(path,{headers});expect(response.ok(),await response.text()).toBe(true);return response.json() as Promise<T>}
  const runtime=<T>(path:string)=>get<T>(config.runtimeUrl+'/gateway/v1'+path)
  const api=<T>(path:string)=>get<T>(config.baseUrl+'/api/v1'+path)
  const save=(name:string,data:unknown)=>writeFileSync(file(name),JSON.stringify(data,null,2)+'\n')
  const report:{passed:boolean;batches:Batch[];formalBatchPosts:number;unexpectedInvocations:string[];checks:unknown[];interactions:unknown[];errors:string[];csv?:unknown}={passed:false,batches:[],formalBatchPosts:0,unexpectedInvocations:[],checks:[],interactions:[],errors}
  try{
    await page.goto(`${config.baseUrl}/applications/${scenario.applicationId}?tab=ui`)
    await page.getByRole('tab',{name:'检查结果',exact:true}).click()
    const checks=['static','preview'].map(mode=>page.waitForResponse(response=>response.url().endsWith(`/ui/drafts/${scenario.draftId}/checks`)&&response.request().postDataJSON()?.mode===mode))
    await page.getByRole('button',{name:'重新检查',exact:true}).click()
    for(const checked of await Promise.all(checks)){
      expect(checked.ok(),await checked.text()).toBe(true)
      const check=checked.request().postDataJSON() as {errors:unknown[];htmlHash:string;formalRequests:number}
      report.checks.push(check);save('batch-browser-verification.json',report)
      expect(check.htmlHash).toBe(scenario.htmlHash);expect(check.errors).toEqual([]);expect(check.formalRequests).toBe(0)
    }
    const head=await api<{revision:number;activePublicationId:string|null;publications:Array<{id:string;manifest:{htmlHash:string}|null}>}>(`/applications/${scenario.applicationId}/ui/publications`)
    const current=head.publications.find(value=>value.id===head.activePublicationId)
    if(current?.manifest?.htmlHash===scenario.htmlHash)scenario.publicationId=current.id
    else{
      const draft=await api<{revision:number}>(`/applications/${scenario.applicationId}/ui/drafts/${scenario.draftId}`)
      const accepted=await page.request.post(`${config.baseUrl}/api/v1/applications/${scenario.applicationId}/ui/publications`,{headers,data:{draftId:scenario.draftId,uiVersionId:null,expectedDraftRevision:draft.revision,expectedPublicationRevision:head.revision,slug:scenario.slug,enabled:true}})
      expect(accepted.ok(),await accepted.text()).toBe(true);scenario.publicationId=(await accepted.json()).id
    }
    await expect.poll(async()=>(await api<{activePublicationId:string}>(`/applications/${scenario.applicationId}/ui/publications`)).activePublicationId,{timeout:120_000}).toBe(scenario.publicationId)
    save('batch-scenario.json',scenario)
    await page.goto(`${config.baseUrl}/a/${scenario.slug}`)
    await expect(page.getByTestId('application-business-shell')).toBeVisible()
    let frame=await businessFrame(page)
    await expect(frame.locator('[data-action="create-batch"]')).toBeVisible()
    await page.screenshot({path:info.outputPath('expense-batch-entry.jpg'),type:'jpeg',fullPage:true})

    async function task(id:string){return runtime<Task>(`/tasks/${id}`)}
    async function waitForTask(id:string,state:string){let latest:Task|undefined;await expect.poll(async()=>{latest=await task(id);return latest.state},{timeout:120_000,intervals:[500,1000]}).toBe(state);return latest!}
    async function supplement(item:Item,number:number){
      const id=item.task!.taskId
      await page.goto(`${config.baseUrl}/a/${scenario.slug}?taskId=${id}`)
      await expect(page.getByTestId('application-current-task')).toHaveText(id)
      const waiting=await waitForTask(id,'waiting_input'),wait=waiting.pendingInputs[0]!
      const business=await businessFrame(page),form=business.locator(`form[data-wait-id="${wait.waitId}"]`)
      await expect(form).toBeVisible()
      const input={purpose:`本地批量演示费用用途 ${number}，仅为虚构测试数据`,invoice_no:`FP-DEMO-${number.toString().padStart(3,'0')}`}
      await fillFields(form,input)
      await page.waitForTimeout(2300)
      await expect(form.locator('[data-field-path="/purpose"]')).toHaveValue(input.purpose)
      const saving=page.waitForResponse(response=>response.url().endsWith(`/invocations/${id}/waits/${wait.waitId}/input`)&&response.request().method()==='POST')
      await form.locator('[data-action="submit-input"]').click()
      const saved=await saving;expect(saved.ok(),await saved.text()).toBe(true)
      const approval=await waitForTask(id,'waiting_approval')
      report.interactions.push({taskId:id,waitId:wait.waitId,input,approvalId:approval.pendingApprovals[0]!.approvalId})
      save('batch-browser-verification.json',report)
      return approval.pendingApprovals[0]!.approvalId
    }
    async function decide(approvalId:string,decision:'approved'|'rejected'){
      let view:{buttons:Array<{id:string;label:string}>}|undefined
      await expect.poll(async()=>{const response=await page.request.get(`${config.baseUrl}/api/v1/approvals/${approvalId}`,{headers});if(response.status()===404)return false;expect(response.ok(),await response.text()).toBe(true);view=await response.json();return true},{timeout:60_000}).toBe(true)
      await page.goto(`${config.baseUrl}/approvals/${approvalId}`)
      const claiming=page.waitForResponse(response=>response.url().endsWith(`/approvals/${approvalId}/claim`)&&response.request().method()==='POST')
      await page.getByRole('button',{name:'领取',exact:true}).click();const claimed=await claiming;expect(claimed.ok(),await claimed.text()).toBe(true)
      const label=view!.buttons.find(value=>value.id===decision)!.label
      await page.getByRole('button',{name:label,exact:true}).click()
      const dialog=page.getByRole('dialog',{name:label,exact:true})
      await dialog.getByLabel('理由（可选）').fill('本地批量功能验收，所有费用均为虚构数据。')
      const deciding=page.waitForResponse(response=>response.url().endsWith(`/approvals/${approvalId}/decide`)&&response.request().method()==='POST')
      await dialog.getByRole('button',{name:label,exact:true}).click();const decided=await deciding;expect(decided.ok(),await decided.text()).toBe(true)
      await expect(dialog).toHaveCount(0)
    }
    const posts:Array<{url:string;body:unknown}>=[]
    page.on('request',request=>{
      if(request.method()!=='POST')return
      if(/\/pages\/[^/]+\/batches$/.test(new URL(request.url()).pathname))posts.push({url:request.url(),body:request.postDataJSON()})
      if(/\/pages\/[^/]+\/invocations$/.test(new URL(request.url()).pathname))report.unexpectedInvocations.push(request.url())
    })
    for(let round=0;round<2;round++){
      await page.goto(`${config.baseUrl}/a/${scenario.slug}`);frame=await businessFrame(page)
      await frame.locator('[data-action="batch-mode"]').click()
      const marker=crypto.randomUUID().slice(0,6)
      const input=Array.from({length:3},(_,index)=>({request_id:`BX-DEMO-${round?'B':'A'}-${index+1}-${marker}`,applicant:['示例员工甲','示例员工乙','示例员工丙'][index]!,expense_type:['差旅费','办公采购','客户招待'][index]!,amount:round?[68.5,144,369.9][index]!:[268.5,135.75,640][index]!}))
      const name=round?'11 月费用报销 · 待办理示例':'11 月费用报销 · 已完成示例'
      await frame.getByLabel('批次名称',{exact:true}).fill(name)
      const priorPosts=posts.length
      if(round){
        await frame.getByRole('button',{name:'导入 CSV / TSV',exact:true}).click()
        const csv='单号,申请人,类型,金额\n'+input.map(row=>[row.request_id,row.applicant,row.expense_type,row.amount].join(',')).join('\n')+'\n'
        await frame.locator('[data-import-text]').fill(csv)
        const parsing=page.waitForResponse(response=>response.url().endsWith('/batch-imports/parse')&&response.request().method()==='POST')
        await frame.locator('[data-action="parse-table"]').click();const parsed=await parsing;expect(parsed.ok(),await parsed.text()).toBe(true)
        for(const[index,field]of ['request_id','applicant','expense_type','amount'].entries())await frame.locator(`[data-column-field="/${field}"]`).selectOption(String(index))
        await frame.locator('[data-action="apply-table"]').click()
      }else{
        const rows=frame.locator('[data-batch-row]'),ids=await rows.evaluateAll(elements=>elements.map(element=>element.getAttribute('data-client-item-id')))
        await rows.nth(1).locator('[data-action="remove-batch-item"]').click()
        await expect(rows).toHaveCount(2)
        expect(await rows.evaluateAll(elements=>elements.map(element=>element.getAttribute('data-client-item-id')))).toEqual([ids[0],ids[2]])
        await frame.locator('[data-action="add-batch-item"]').click()
        for(let index=0;index<3;index++)await fillFields(rows.nth(index),input[index])
      }
      const rows=frame.locator('[data-batch-row]');await expect(rows).toHaveCount(3)
      for(let index=0;index<3;index++)await expect(rows.nth(index).locator('[data-field-path="/request_id"]')).toHaveValue(input[index]!.request_id)
      expect(posts.length,'Preparing rows and import cannot create business work').toBe(priorPosts)
      const clientIds=await rows.evaluateAll(elements=>elements.map(element=>element.getAttribute('data-client-item-id')))
      const accepting=page.waitForResponse(response=>response.url().endsWith(`/pages/${scenario.slug}/batches`)&&response.request().method()==='POST')
      await frame.locator('[data-action="create-batch"]').click()
      const accepted=await accepting;expect(accepted.status(),await accepted.text()).toBe(202)
      const batch=await accepted.json() as Batch;expect(batch.counts.total).toBe(3)
      report.batches.push(batch);report.formalBatchPosts=posts.length;save('batch-browser-verification.json',report)
      await expect(page.getByTestId('application-current-batch')).toHaveText(batch.batchId)
      await page.reload();await expect(page.getByTestId('application-current-batch')).toHaveText(batch.batchId)
      let items:Item[]=[]
      await expect.poll(async()=>{items=(await runtime<{items:Item[]}>(`/batches/${batch.batchId}/items`)).items;return items.length===3&&items.every(value=>value.state==='waiting_input')},{timeout:120_000,intervals:[500,1000]}).toBe(true)
      expect(items.map(value=>value.input)).toEqual(input);expect(items.map(value=>value.clientItemId)).toEqual(clientIds)
      const restored=await businessFrame(page);await expect(restored.locator('#countInput')).toHaveText('3')
      await expect(restored.locator('#batchItems tr')).toHaveCount(3)
      // First batch completes all three original tasks; second deliberately
      // retains one input wait and one approval for the user to operate.
      const process=round?[0,1]:[0,1,2]
      for(const index of process){
        const approvalId=await supplement(items[index]!,round*3+index+1)
        if(!round||index===0){
          const decision=!round&&index===2?'rejected':'approved'
          await decide(approvalId,decision)
          const completed=await waitForTask(items[index]!.task!.taskId,'completed')
          expect(completed.outputs).toMatchObject({...input[index],decision})
        }
      }
      await expect.poll(async()=>{const current=await runtime<Batch>(`/batches/${batch.batchId}`);return current.counts.completed},{timeout:120_000,intervals:[500,1000]}).toBe(round?1:3)
      const current=await runtime<Batch>(`/batches/${batch.batchId}`);report.batches[round]=current
      if(round){expect(current.counts.waitingInput).toBe(1);expect(current.counts.waitingApproval).toBe(1)}
      await page.goto(`${config.baseUrl}/a/${scenario.slug}?batchId=${batch.batchId}`)
      frame=await businessFrame(page);await expect(frame.locator('#batchItems tr')).toHaveCount(3)
      await expect(frame.locator('#countCompleted')).toHaveText(round?'1':'3')
      for(const value of input)await expect(frame.getByText(value.request_id,{exact:true})).toBeVisible()
      if(!round){
        await frame.locator('[data-action="export-batch"]').click()
        const downloadButton=frame.locator('[data-action="download-batch-export"]');await expect(downloadButton).toBeVisible({timeout:60_000})
        const downloading=page.waitForEvent('download');await downloadButton.click();const download=await downloading
        const destination=file('expense-batch-completed.csv');await download.saveAs(destination)
        const csv=readFileSync(destination,'utf8');for(const value of input)expect(csv).toContain(value.request_id)
        report.csv={file:destination,bytes:Buffer.byteLength(csv),allThreeRowsPresent:true}
      }
      await page.screenshot({path:info.outputPath(round?'expense-batch-waiting.jpg':'expense-batch-completed.jpg'),type:'jpeg',fullPage:true})
      save('batch-browser-verification.json',report)
    }
    await page.goto(`${config.baseUrl}/applications/${scenario.applicationId}?tab=invocations&recordTab=batch`)
    await expect(page.getByTestId('application-batches')).toBeVisible()
    for(const batch of report.batches)await expect(page.getByText(batch.name,{exact:true})).toBeVisible()
    await page.screenshot({path:info.outputPath('expense-batch-list.jpg'),type:'jpeg',fullPage:true})
    expect(posts).toHaveLength(2);expect(report.unexpectedInvocations).toEqual([]);expect(errors).toEqual([])
    report.passed=true;report.formalBatchPosts=posts.length
  }finally{save('batch-browser-verification.json',report)}
})
