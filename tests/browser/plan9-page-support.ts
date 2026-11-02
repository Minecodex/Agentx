import { expect, type Browser, type BrowserContext, type Frame, type Locator, type Page, type TestInfo } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { useRuntimePortForward } from './tests/playground-helpers'
import { clickForResponse, trackAccessToken } from './browser-http'
type AccessToken=ReturnType<typeof trackAccessToken>

export type Decision={decision:string;actor:string;reason?:string;nodeId?:string}
export type Scenario={scene:string;applicationId:string;slug:string;inputs:Record<string,unknown>[];expected:Record<string,unknown>[];
  roundApplications?:Array<{applicationId:string;deploymentId:string;slug:string}>;
  waitInputs?:Array<Record<string,unknown[]>>;approvalDecisions?:Decision[][];simultaneousWaits?:number;repeatNode?:string;
  fileOutputs?:boolean;downloadText?:string[];generatedFields?:string[];crm?:boolean;chat?:boolean;
  followups?:Array<Array<{message:string;answer:string}>>;cancellation?:boolean;intermediateMessages?:string[];
  creationChain?:boolean;pageAttemptBudget?:number;steeringTitle?:string}
export type Task={taskId:string;state:string;sessionId:string|null;outputs:Record<string,unknown>;pendingInputs:Array<{waitId:string;nodePath:string;occurrence:number}>;
  pendingApprovals:Array<{approvalId:string}>;messages:Array<{content:unknown}>;files:Array<{artifactId:string;sha256:string}>;allowedActions:{canCancel:boolean};[key:string]:unknown}

// A verified submitted input reached the real Runtime. Changing HTML cannot
// repair its frozen workflow; the Python chain driver must consult Canvas AI.
export class WorkflowExecutionFailure extends Error{
  constructor(readonly task:Task,readonly diagnostic:string){super(`PLAN9_WORKFLOW_EXECUTION_FAILURE: ${diagnostic}`)}
}

const options=()=>({baseURL:process.env.AGENTX_E2E_BASE_URL,locale:'zh-CN',viewport:{width:1440,height:900}})
export async function loginAt(page:Page,path:string,username:string,password:string){
  await useRuntimePortForward(page);await page.goto(path)
  await page.getByLabel('用户名').fill(username);await page.getByLabel('密码').fill(password)
  const response=await clickForResponse(page,page.getByRole('button',{name:'登录',exact:true}),response=>response.url().endsWith('/api/v1/auth/login')&&response.request().method()==='POST');expect(response.ok(),await response.text()).toBe(true)
  return (await response.json()).accessToken as string
}
export async function businessFrame(page:Page){
  let result:Frame|undefined
  await expect.poll(()=>{result=page.frames().find(frame=>frame.name()==='agentx-application-page');return Boolean(result)},{timeout:30_000}).toBe(true)
  return result!
}
export async function downloadPageFile(page:Page,frame:Frame){
  const action=frame.locator('[data-action="download-file"]').first()
  await action.waitFor({state:'visible'})
  // Both promises need handlers immediately: a missing/failed action must stay
  // a normal page validation failure so its bounded AI repair loop can continue.
  const [file]=await Promise.all([page.waitForEvent('download'),action.click()])
  return file
}
export function waitInputScope(frame:Frame,waitId:string){
  const container=`[data-wait-id="${waitId}"]:not(button, a, input, textarea, select)`
  const action='[data-action="submit-input"]'
  // Correlated wrappers identify one form; independent forms stay ambiguous.
  const owner=`${container}:is(form${action}, :has(${action}))`
  return frame.locator(`${owner}:not(:has(${owner}))`)
}
export async function waitInputAction(scope:Locator){
  const marked=scope.locator('[data-action="submit-input"]:not(:has([data-action="submit-input"]))')
  if(await marked.count()||await scope.getAttribute('data-action')!=='submit-input')return marked
  return scope.locator('button[type="submit"],button:not([type]),input[type="submit"]')
}
export async function fillFields(scope:Frame|Locator,input:unknown,path=''):Promise<void>{
  if(input&&typeof input==='object'&&'__file' in input){
    const file=(input as {__file:{name:string;mimeType:string;content:string}}).__file
    await scope.locator(`input[type="file"][data-field-path="${path}"]`).setInputFiles({name:file.name,mimeType:file.mimeType,buffer:Buffer.from(file.content)})
  }else if(Array.isArray(input)){
    const upload=scope.locator(`[data-field-path="${path}"][type="file"][multiple]`)
    if(input.every(value=>value&&typeof value==='object'&&'__file' in value)&&await upload.count()){
      await upload.setInputFiles(input.map(value=>({name:value.__file.name,mimeType:value.__file.mimeType,buffer:Buffer.from(value.__file.content)})))
      return
    }
    for(let index=0;index<input.length;index++){
      if(!await scope.locator(`[data-field-path="${path}/${index}"], [data-field-path^="${path}/${index}/"]`).count())await scope.locator(`[data-action="add-item"][data-field-path="${path}"]`).click()
      await fillFields(scope,input[index],`${path}/${index}`)
    }
  }else if(input&&typeof input==='object'){
    for(const [key,value]of Object.entries(input))await fillFields(scope,value,`${path}/${key.replaceAll('~','~0').replaceAll('/','~1')}`)
  }else{
    const field=scope.locator(`input[data-field-path="${path}"],textarea[data-field-path="${path}"],select[data-field-path="${path}"]`).first()
    await expect(field,`Missing business field ${path}`).toBeVisible()
    const tag=await field.evaluate(element=>element.tagName)
    if(tag==='SELECT')await field.selectOption(String(input))
    else if(await field.getAttribute('type')==='radio')await scope.locator(`input[type="radio"][data-field-path="${path}"][value=${JSON.stringify(String(input))}]`).check()
    else if(typeof input==='boolean')await field.setChecked(input)
    else await field.fill(String(input??''))
  }
}

export class Reviewers{
  private sessions=new Map<string,{context:BrowserContext;page:Page;token:AccessToken}>()
  constructor(private browser:Browser,private info:TestInfo){}
  async decide(id:string,decision:Decision,number:number,scenario:Scenario){
    let session=this.sessions.get(decision.actor)
    if(!session){
      const users=JSON.parse(process.env.AGENTX_E2E_PLAN9_REVIEWERS??'{}') as Record<string,{username:string;password:string}>
      const actor=users[decision.actor];expect(actor,`Missing reviewer ${decision.actor}`).toBeTruthy()
      const context=await this.browser.newContext(options()),page=await context.newPage()
      const token=trackAccessToken(page,await loginAt(page,`/approvals/${id}`,actor.username,actor.password))
      session={context,page,token};this.sessions.set(decision.actor,session)
    }
    const {page,token}=session
    let view:{buttons:Array<{id:string;label:string}>;subject:{kind:string;effectId?:string;argumentsHash?:string};requestPayload:{effectId?:string;argumentsHash?:string;arguments?:Record<string,unknown>}}|undefined
    await expect.poll(async()=>{const response=await page.request.get(`/api/v1/approvals/${id}`,{headers:{Authorization:`Bearer ${await token()}`}});if(response.status()===404)return false;expect(response.ok(),await response.text()).toBe(true);view=await response.json();return true},{timeout:60_000}).toBe(true)
    await page.goto(`/approvals/${id}`)
    if(scenario.crm){
      await expect(page.getByTestId('tool-effect-approval')).toBeVisible();expect(view!.subject.kind).toBe('agent_tool_effect')
      const payload=view!.requestPayload,args=payload.arguments!
      const input=scenario.inputs.find(value=>value.request_id===args.reference)
      expect(input,'The approval must target this submitted business request').toBeTruthy()
      expect(args).toMatchObject({reference:input!.request_id,customer:input!.customer,grossAmount:input!.grossAmount,discountPercent:scenario.scene==='S18'&&decision.decision==='approved'?5:input!.discountPercent})
      expect(payload.effectId).toBe(view!.subject.effectId);expect(payload.argumentsHash).toBe(view!.subject.argumentsHash)
    }
    const claimed=await clickForResponse(page,page.getByRole('button',{name:'领取',exact:true}),response=>response.url().endsWith(`/approvals/${id}/claim`)&&response.request().method()==='POST');expect(claimed.ok(),await claimed.text()).toBe(true)
    const storedLabel=view!.buttons.find(button=>button.id===decision.decision)?.label
    const localized=decision.decision==='approved'?'批准':'拒绝'
    const label=scenario.crm&&await page.getByRole('button',{name:localized,exact:true}).count()?localized:storedLabel
    expect(label).toBeTruthy()
    await page.screenshot({path:this.info.outputPath(`approval-${number}-${decision.decision}.png`),fullPage:true})
    await page.getByRole('button',{name:label!,exact:true}).click()
    const dialog=page.getByRole('dialog',{name:label!,exact:true})
    if(decision.reason)await dialog.getByLabel('理由（可选）').fill(decision.reason)
    const settled=await clickForResponse(page,dialog.getByRole('button',{name:label!,exact:true}),response=>response.url().endsWith(`/approvals/${id}/decide`)&&response.request().method()==='POST');expect(settled.ok(),await settled.text()).toBe(true)
    return {approvalId:id,decision,subject:view!.subject,requestPayload:view!.requestPayload,result:await settled.json()}
  }
  async close(){for(const session of this.sessions.values())await session.context.close()}
}

async function task(page:Page,token:AccessToken,id:string){
  const response=await page.request.get(`${process.env.AGENTX_E2E_RUNTIME_URL}/gateway/v1/tasks/${id}`,{headers:{Authorization:`Bearer ${await token()}`}})
  expect(response.ok(),await response.text()).toBe(true)
  return await response.json() as Task
}
async function crmRecords(page:Page,reference:unknown){
  const response=await page.request.get(`${process.env.AGENTX_E2E_PLAN9_CRM_URL}/v1/plan9/quotes`)
  expect(response.ok(),await response.text()).toBe(true)
  return (await response.json() as Array<Record<string,unknown>>).filter(item=>item.reference===reference)
}
async function settle(page:Page,token:AccessToken,id:string,scenario:Scenario,index:number,reviewers:Reviewers,evidence:unknown[],beforeWrites:number){
  const waits=new Set<string>(),approvals=new Set<string>(),occurrences=new Map<string,number>()
  const seenWaits:unknown[]=[],decisions= scenario.approvalDecisions?.[index]??[]
  let simultaneous=false,current:Task|undefined,closedInput:number|undefined,reopened=false
  await expect.poll(async()=>{
    current=await task(page,token,id)
    if(['completed','failed','cancelled','outcome_unknown'].includes(current.state))return true
    if(scenario.simultaneousWaits&&!simultaneous&&current.pendingInputs.length!==scenario.simultaneousWaits)return false
    if(!reopened&&(current.pendingInputs.length||current.pendingApprovals.length||(scenario.intermediateMessages&&current.messages.length))){
      await page.reload();await expect(page.getByTestId('application-current-task')).toHaveText(id)
      reopened=true;evidence.push({reopenedPendingTask:id,state:current.state});return false
    }
    if(current.pendingInputs.length){
      if(current.pendingInputs.length===(scenario.simultaneousWaits??0))simultaneous=true
      for(const wait of current.pendingInputs){
        if(waits.has(wait.waitId))continue
        const ordinal=occurrences.get(wait.nodePath)??0
        const content=scenario.waitInputs?.[index]?.[wait.nodePath]?.[ordinal]
        expect(content,`No unseen input configured for ${wait.nodePath} occurrence ${ordinal}`).toBeDefined()
        const frame=await businessFrame(page),scope=waitInputScope(frame,wait.waitId)
        await expect(scope).toBeVisible({timeout:30_000});await fillFields(scope,content)
        const submit=await waitInputAction(scope)
        const receipt=await clickForResponse(page,submit,response=>response.url().endsWith(`/invocations/${id}/waits/${wait.waitId}/input`)&&response.request().method()==='POST');expect(receipt.ok(),await receipt.text()).toBe(true)
        waits.add(wait.waitId);occurrences.set(wait.nodePath,ordinal+1);seenWaits.push({waitId:wait.waitId,nodePath:wait.nodePath,content})
        if(scenario.repeatNode&&ordinal===0){
          const replay=await page.request.post(`${process.env.AGENTX_E2E_RUNTIME_URL}/gateway/v1/invocations/${id}/waits/${wait.waitId}/input`,{headers:{Authorization:`Bearer ${await token()}`,'Idempotency-Key':crypto.randomUUID()},data:{content}})
          closedInput=replay.status();expect(closedInput).toBe(409)
        }
      }
    }
    for(const approval of current.pendingApprovals){
      if(approvals.has(approval.approvalId))continue
      const decision=decisions[approvals.size];expect(decision,'Unexpected approval stage').toBeTruthy()
      if(scenario.crm)expect((await crmRecords(page,scenario.inputs[index]!.request_id)).length,'Zero external writes before concrete approval').toBe(beforeWrites)
      const value=await reviewers.decide(approval.approvalId,decision!,approvals.size,scenario)
      approvals.add(approval.approvalId);evidence.push(value)
    }
    return ['completed','failed','cancelled','outcome_unknown'].includes(current.state)
  },{timeout:360_000,intervals:[500,1000,2000]}).toBe(true)
  if(current!.state==='outcome_unknown')throw new Error(`PLAN9_BUSINESS_OUTCOME_UNKNOWN: preserve original task ${id}; do not repeat a business operation with an unverified write result`)
  if(current!.state==='completed'){
    expect(approvals.size,'All requested approval stages must be exercised').toBe(decisions.length)
    if(scenario.simultaneousWaits)expect(simultaneous,'Both input waits must coexist').toBe(true)
  }
  evidence.push({inputWaits:seenWaits,closedInput})
  return current!
}

async function displayed(frame:Frame,expected:Record<string,unknown>){
  for(const value of Object.values(expected)){
    if(value&&typeof value==='object')await displayed(frame,value as Record<string,unknown>)
    if(typeof value==='string'&&value)await expect(frame.locator('body')).toContainText(value,{timeout:30_000})
    if(typeof value==='number')await expect.poll(async()=>((await frame.locator('body').innerText()).replaceAll(',','').match(/-?\d+(?:\.\d+)?/g)??[]).map(Number)).toContain(value)
  }
}

function submittedExpectation(value:unknown):unknown{
  if(Array.isArray(value))return value.map(submittedExpectation)
  if(value&&typeof value==='object'){
    if('__file'in value){const file=(value as {__file:{content:string;mimeType:string}}).__file;return{contentType:file.mimeType,sizeBytes:Buffer.byteLength(file.content),sha256:createHash('sha256').update(file.content).digest('hex')}}
    return Object.fromEntries(Object.entries(value).map(([key,item])=>[key,submittedExpectation(item)]))
  }
  return value
}

export async function executeCase(browser:Browser,scenario:Scenario,suffix:string,index:number,info:TestInfo,attempt:number){
  const began=Date.now()
  const context=await browser.newContext(options()),page=await context.newPage(),reviewers=new Reviewers(browser,info)
  const evidence:unknown[]=[]
  const browserErrors:string[]=[]
  page.on('pageerror',error=>{if(browserErrors.length<10)browserErrors.push(error.message.slice(0,1500))})
  let token=()=>Promise.resolve(''),acceptedTaskId:string|undefined,lastTask:Task|undefined
  try{
    token=trackAccessToken(page,await loginAt(page,`/a/${suffix}`,process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!))
    await expect(page.getByTestId('application-business-shell')).toBeVisible()
    let frame=await businessFrame(page)
    if(scenario.steeringTitle)await expect(frame.getByRole('heading',{name:scenario.steeringTitle,exact:true}).first()).toBeVisible()
    const acceptedTasks:Promise<string>[]=[]
    const isSubmission=(url:string)=>scenario.chat?/\/sessions\/[^/]+\/messages$/.test(url):url.endsWith(`/pages/${suffix}/invocations`)
    page.on('response',response=>{
      if(isSubmission(response.url())&&response.request().method()==='POST'&&response.ok())acceptedTasks.push(response.json().then(value=>scenario.chat?value.id:value.taskId))
    })
    if(scenario.scene==='S03'&&index===0){
      await fillFields(frame,{...scenario.inputs[index],order_id:'',packages:0})
      const invalidAction=frame.locator('[data-action="invoke"]')
      if(await invalidAction.isEnabled())await invalidAction.click()
      // Native validation, page validation and the formal SDK may all reject;
      // allow the actual request a chance to run, but no Invocation may be accepted.
      await page.waitForTimeout(400)
      expect(acceptedTasks,'An invalid order must not create a business task').toHaveLength(0)
      evidence.push({requiredAndNumericBoundaryRejected:true})
    }
    await fillFields(frame,scenario.inputs[index])
    if(scenario.scene==='S02'&&index===1){
      const length=(scenario.inputs[index]!.items as unknown[]).length
      await frame.locator('[data-action="add-item"][data-field-path="/items"]').click()
      await expect(frame.locator(`[data-field-path="/items/${length}/name"]`)).toBeVisible()
      await frame.locator(`[data-action="remove-item"][data-field-path="/items/${length}"]`).click()
      await expect(frame.locator(`[data-field-path="/items/${length}/name"]`)).toHaveCount(0)
      evidence.push({arrayAdditionAndRemoval:true,remainingRows:length})
    }
    const before=scenario.crm?(await crmRecords(page,scenario.inputs[index]!.request_id)).length:0
    const streams:Array<{url:string;cursor:string|undefined}>=[]
    if(scenario.scene==='S20'&&index===0){
      await page.route('**/gateway/v1/invocations/*/events',async route=>{
        if(route.request().method()!=='GET'){await route.continue();return}
        streams.push({url:route.request().url(),cursor:route.request().headers()['last-event-id']})
        if(streams.length===1){
          await route.fulfill({status:200,headers:{'Content-Type':'text/event-stream','Cache-Control':'no-store','Access-Control-Allow-Origin':new URL(process.env.AGENTX_E2E_BASE_URL!).origin,'Access-Control-Allow-Credentials':'true'},body:': controlled connection close\n\n'})
        }else await route.continue()
      })
    }
    const action=frame.locator(scenario.chat?'[data-action="send-message"]':'[data-action="invoke"]')
    await action.waitFor({state:'visible'})
    const [accepted]=await Promise.all([page.waitForResponse(response=>isSubmission(response.url())&&response.request().method()==='POST'),scenario.scene==='S01'&&index===1?action.dblclick():action.click()])
    expect(accepted.ok(),await accepted.text()).toBe(true)
    const initial=await accepted.json(),id=scenario.chat?initial.id:initial.taskId;acceptedTaskId=id
    const sentInput=accepted.request().postDataJSON()
    if(scenario.chat)expect(sentInput.parts.find((part:{partType:string})=>part.partType==='text')?.content).toBe(scenario.inputs[index]!.message)
    else expect(sentInput.input,'The generated page must preserve the business input and uploaded bytes').toMatchObject(submittedExpectation(scenario.inputs[index]) as Record<string,unknown>)
    if(scenario.scene==='S20'&&index===0){
      await expect.poll(()=>streams.length,{message:'Long report: the SDK subscription must reconnect the closed event stream for the same task before refresh',timeout:20_000,intervals:[100,250,500]}).toBeGreaterThanOrEqual(2)
      expect(streams.slice(0,2).every(stream=>new URL(stream.url).pathname.endsWith(`/invocations/${id}/events`))).toBe(true)
      expect(streams.slice(0,2).every(stream=>/^\d+$/.test(stream.cursor??''))).toBe(true)
      expect(Number(streams[1]!.cursor)).toBeGreaterThanOrEqual(Number(streams[0]!.cursor))
      evidence.push({sseRecovery:{closedConnections:1,taskId:id,requests:streams.slice(0,2)}})
    }
    const finished=await settle(page,token,id,scenario,index,reviewers,evidence,before)
    expect([...new Set(await Promise.all(acceptedTasks))],'One form submission must create only its original task').toEqual([id])
    lastTask=finished
    if(scenario.creationChain&&finished.state==='failed')throw new WorkflowExecutionFailure(finished,'The real workflow failed after the generated page submitted the verified business input.')
    expect(finished.state,JSON.stringify(finished)).toBe('completed')
    try{
      if(scenario.fileOutputs||scenario.generatedFields)expect(finished.outputs).toMatchObject(scenario.expected[index]!)
      else expect(finished.outputs).toEqual(scenario.expected[index])
    }catch(error){
      if(scenario.creationChain)throw new WorkflowExecutionFailure(finished,'The real workflow outputs did not satisfy the held-out business assertion. Business values remain hidden from the repair prompt.')
      throw error
    }
    frame=await businessFrame(page);await displayed(frame,scenario.expected[index]!)
    if(scenario.crm){
      const records=await crmRecords(page,scenario.inputs[index]!.request_id)
      expect(records.length,'One approved operation must cause exactly one external write').toBe(before+1)
      expect(records.at(-1)).toEqual({receiptId:finished.outputs.receipt_id,customer:finished.outputs.customer,total:finished.outputs.total,discountPercent:finished.outputs.discount_percent,reference:finished.outputs.reference})
      evidence.push({crmReceipt:records.at(-1),writesBefore:before,writesAfter:records.length})
    }
    if(scenario.fileOutputs){
      expect(finished.files.length).toBeGreaterThan(0)
      const file=await downloadPageFile(page,frame),path=info.outputPath(`report-attempt${attempt}-case${index}.txt`)
      await file.saveAs(path)
      if(scenario.downloadText)expect(readFileSync(path,'utf8')).toBe(scenario.downloadText[index])
      else expect(readFileSync(path,'utf8')).toContain(String(finished.outputs.receipt_id))
    }
    if(scenario.chat){
      await page.reload();await expect(page.getByTestId('application-current-task')).toHaveText(finished.taskId)
      frame=await businessFrame(page);evidence.push({reopenedConversation:finished.sessionId,taskId:finished.taskId})
    }
    for(const followup of scenario.followups?.[index]??[]){
      await fillFields(frame,{message:followup.message})
      const response=await clickForResponse(page,frame.locator('[data-action="send-message"]'),response=>/\/sessions\/[^/]+\/messages$/.test(response.url())&&response.request().method()==='POST');expect(response.ok(),await response.text()).toBe(true)
      const nextId=(await response.json()).id;acceptedTaskId=nextId;lastTask=undefined
      expect(response.request().postDataJSON().parts.find((part:{partType:string})=>part.partType==='text')?.content).toBe(followup.message)
      const next=await settle(page,token,nextId,{...scenario,approvalDecisions:undefined,waitInputs:undefined},index,reviewers,evidence,0)
      lastTask=next
      expect(next.state).toBe('completed');expect(next.sessionId).toBe(finished.sessionId);expect(next.outputs.answer).toBe(followup.answer)
      await displayed(frame,{answer:followup.answer});evidence.push({followup:next})
    }
    if(scenario.intermediateMessages)for(const message of scenario.intermediateMessages){expect(JSON.stringify(finished.messages)).toContain(message);await expect(frame.locator('body')).toContainText(message)}
    await page.screenshot({path:info.outputPath(`business-attempt${attempt}-input${index}.png`),fullPage:true})
    await page.reload();await expect(page.getByTestId('application-current-task')).toBeVisible()
    frame=await businessFrame(page)
    const restoredExpected=scenario.followups?.[index]?.at(-1)?.answer
    await displayed(frame,restoredExpected?{answer:restoredExpected}:scenario.expected[index]!)
    if(scenario.cancellation&&index===1){
      await page.getByTestId('application-new-task').click()
      frame=await businessFrame(page)
      const cancelReference=`${scenario.inputs[index]!.request_id}-cancel-${crypto.randomUUID().slice(0,8)}`
      await fillFields(frame,{...scenario.inputs[index],request_id:cancelReference})
      const response=await clickForResponse(page,frame.locator('[data-action="invoke"]'),response=>response.url().endsWith(`/pages/${suffix}/invocations`)&&response.request().method()==='POST');expect(response.ok(),await response.text()).toBe(true)
      const id=(await response.json()).taskId;acceptedTaskId=id;lastTask=undefined
      await expect.poll(async()=>(await task(page,token,id)).messages.length,{timeout:60_000}).toBeGreaterThan(0)
      const receipt=await clickForResponse(page,frame.locator('[data-action="cancel"]'),response=>response.url().endsWith(`/invocations/${id}/cancel`)&&response.request().method()==='POST');expect(receipt.ok(),await receipt.text()).toBe(true)
      let final:Task|undefined
      await expect.poll(async()=>{final=await task(page,token,id);return ['cancelled','outcome_unknown'].includes(final.state)},{timeout:60_000}).toBe(true)
      expect(final!.allowedActions.canCancel).toBe(false)
      if(final!.state==='outcome_unknown'){
        // An interrupted arbitrary Sandbox program is not proof of cancellation.
        // Preserve that original task, show the uncertainty, and never replay it.
        await expect(frame.locator('body')).toContainText(/结果未知|待核实|outcome_unknown|状态未知/)
        expect(final!.files).toEqual([])
      }
      expect([...new Set(await Promise.all(acceptedTasks))]).toEqual([finished.taskId,id])
      evidence.push({cancelledTask:final,cancelReference,cancellationConfirmed:final!.state==='cancelled',originalTaskNotReplayed:true})
      await page.screenshot({path:info.outputPath(`cancelled-attempt${attempt}.png`),fullPage:true})
    }
    return {input:scenario.inputs[index],expected:scenario.expected[index],task:finished,evidence,elapsedMs:Date.now()-began}
  }catch(error){
    if(acceptedTaskId)try{
      lastTask=await task(page,token,acceptedTaskId)
      if(lastTask.allowedActions.canCancel){
        const response=await page.request.post(`${process.env.AGENTX_E2E_RUNTIME_URL}/gateway/v1/invocations/${acceptedTaskId}/cancel`,{headers:{Authorization:`Bearer ${await token()}`,'Idempotency-Key':crypto.randomUUID()},data:{}})
        evidence.push({failedAttemptCancellation:{taskId:acceptedTaskId,status:response.status()}})
        if(response.ok())await expect.poll(async()=>{lastTask=await task(page,token,acceptedTaskId!);return ['completed','failed','cancelled','outcome_unknown'].includes(lastTask.state)},{timeout:30_000}).toBe(true)
      }
    }catch(cleanupError){evidence.push({failedAttemptCleanupError:String(cleanupError)})}
    if(browserErrors.length&&!(error instanceof WorkflowExecutionFailure))throw new Error(`${String(error)}\nBrowser script errors: ${JSON.stringify(browserErrors)}`,{cause:error})
    throw error
  }finally{
    if(acceptedTaskId)lastTask=await task(page,token,acceptedTaskId).catch(()=>lastTask)
    await info.attach(`business-attempt${attempt}-case${index}`,{contentType:'application/json',body:JSON.stringify({input:scenario.inputs[index],expected:scenario.expected[index],task:lastTask,evidence,browserErrors})})
    await reviewers.close();await context.close()
  }
}

export async function verifyHistoricalPage(browser:Browser,_scenario:Scenario,suffix:string,info:TestInfo){
  const context=await browser.newContext(options()),page=await context.newPage()
  const formalMutationRequests:string[]=[]
  page.on('request',request=>{if(!['GET','HEAD','OPTIONS'].includes(request.method())&&new URL(request.url()).pathname.startsWith('/gateway/v1/'))formalMutationRequests.push(request.method()+' '+new URL(request.url()).pathname)})
  try{
    const token=trackAccessToken(page,await loginAt(page,`/a/${suffix}`,process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!))
    await expect(page.getByTestId('application-business-shell')).toBeVisible()
    const headers={Authorization:`Bearer ${await token()}`},root=process.env.AGENTX_E2E_RUNTIME_URL
    const activeResponse=await page.request.get(`${root}/gateway/v1/pages/${suffix}`,{headers})
    expect(activeResponse.ok(),await activeResponse.text()).toBe(true)
    const active=await activeResponse.json()
    const response=await page.request.get(`${root}/gateway/v1/applications/${active.routeKey}/my-tasks?limit=50`,{headers})
    expect(response.ok(),await response.text()).toBe(true)
    const history=(await response.json()).items as Array<Task&{accessBinding?:{kind:string;publicationId:string}}>
    const original=history.find(value=>value.state==='completed'&&value.accessBinding?.kind==='web'&&value.accessBinding.publicationId!==active.publication.publicationId)
    expect(original,'A new page must expose previous page tasks in its own history').toBeTruthy()
    const frame=await businessFrame(page)
    const refresh=frame.locator('[data-action="list-tasks"]')
    if(await refresh.count())await refresh.first().click()
    const item=frame.locator(`[data-task-id="${original!.taskId}"]`).first()
    await expect(item).toBeVisible()
    const button=item.getByRole('button').first()
    if(await button.count())await button.click();else await item.click()
    await expect(page).toHaveURL(new RegExp(`taskId=${original!.taskId}`))
    await expect(page.getByTestId('application-current-task')).toHaveText(original!.taskId)
    await expect.poll(()=>frame.isDetached(),{timeout:30_000}).toBe(true)
    const restored=await page.request.get(`${root}/gateway/v1/pages/${suffix}?taskId=${original!.taskId}`,{headers})
    expect(restored.ok(),await restored.text()).toBe(true)
    const view=await restored.json()
    expect(view.historical).toBe(true)
    expect(view.publication.publicationId).toBe(original!.accessBinding!.publicationId)
    const restoredFrame=await businessFrame(page)
    const fileIds=new Set(original!.files.map(file=>file.artifactId))
    function businessValues(value:unknown):unknown{
      if(Array.isArray(value))return value.map(businessValues).filter(item=>item!==undefined)
      if(value&&typeof value==='object'){
        if('artifactId'in value&&typeof value.artifactId==='string'&&fileIds.has(value.artifactId))return undefined
        return Object.fromEntries(Object.entries(value).map(([key,item])=>[key,businessValues(item)]).filter(([,item])=>item!==undefined))
      }
      return value
    }
    await displayed(restoredFrame,businessValues(original!.outputs) as Record<string,unknown>)
    let downloadSha256:string|undefined
    if(original!.files.length){
      const file=await downloadPageFile(page,restoredFrame),path=info.outputPath('historical-report.bin')
      await file.saveAs(path)
      downloadSha256=createHash('sha256').update(readFileSync(path)).digest('hex')
      expect(original!.files.map(file=>file.sha256),'The old page must download an original authorized file').toContain(downloadSha256)
    }
    await page.screenshot({path:info.outputPath('original-page-history.png'),fullPage:true})
    expect(formalMutationRequests,'Opening an original page in history must not create or mutate a business task').toEqual([])
    return {taskId:original!.taskId,activePublicationId:active.publication.publicationId,restoredPublicationId:view.publication.publicationId,downloadSha256,formalMutationRequests}
  }finally{await context.close()}
}

export async function resumeExistingCase(browser:Browser,scenario:Scenario,suffix:string,id:string,info:TestInfo){
  const context=await browser.newContext(options()),page=await context.newPage(),reviewers=new Reviewers(browser,info)
  const evidence:unknown[]=[]
  try{
    const token=trackAccessToken(page,await loginAt(page,`/a/${suffix}?taskId=${id}`,process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!))
    await expect(page.getByTestId('application-current-task')).toHaveText(id)
    const before=scenario.crm?(await crmRecords(page,scenario.inputs[0]!.request_id)).length:0
    const finished=await settle(page,token,id,scenario,0,reviewers,evidence,before)
    expect(finished.state,JSON.stringify(finished)).toBe('completed')
    expect(finished.outputs).toMatchObject(scenario.expected[0]!)
    const frame=await businessFrame(page)
    await displayed(frame,scenario.expected[0]!)
    if(scenario.crm)expect((await crmRecords(page,scenario.inputs[0]!.request_id)).length).toBe(before+1)
    if(scenario.fileOutputs){
      const file=await downloadPageFile(page,frame),path=info.outputPath('cross-entry-report.txt')
      await file.saveAs(path)
      const bytes=readFileSync(path),sha256=createHash('sha256').update(bytes).digest('hex')
      expect(bytes.toString()).toContain(String(finished.outputs.receipt_id))
      expect(finished.files.some(file=>file.sha256===sha256)).toBe(true)
      evidence.push({downloadSha256:sha256})
    }
    await page.screenshot({path:info.outputPath('personal-mcp-web-result.png'),fullPage:true})
    return {task:finished,evidence}
  }finally{await reviewers.close();await context.close()}
}
