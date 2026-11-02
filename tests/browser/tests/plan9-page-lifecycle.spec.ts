import {errors,expect,test} from '@playwright/test'
import {existsSync,readFileSync,writeFileSync,renameSync} from 'node:fs'
import {join} from 'node:path'
import {login,read} from '../studio-builder-support'
import {businessFrame,fillFields,loginAt,type Scenario,type Task} from '../plan9-page-support'
import {useRuntimePortForward} from './playground-helpers'

type Publication={id:string;uiVersionId:string;revision:number;slug:string;status:string;manifest:{htmlHash:string;enabled:boolean;contract:{bundleId:string}}}
type Publications={revision:number;publications:Publication[]}
const directory=process.env.AGENTX_E2E_PLAN9_LIFECYCLE_DIRECTORY!
const scenario=JSON.parse(process.env.AGENTX_E2E_PLAN9_LIFECYCLE_SCENARIO!) as Scenario

test('AI page rollback preserves pending input and runs with Control offline after a Worker restart',async({page,browser},info)=>{
  test.setTimeout(600_000)
  await useRuntimePortForward(page)
  const admin=await login(page),root=process.env.AGENTX_E2E_RUNTIME_URL
  const listing=()=>read<Publications>(page,admin,`/applications/${scenario.applicationId}/ui/publications`)
  const initial=await listing(),current=initial.publications.find(row=>row.status==='active')!
  expect(current).toBeTruthy()
  const versions=initial.publications.filter(row=>['active','superseded'].includes(row.status))
  const previous=versions.findLast(row=>row.id!==current.id)!
  expect(previous,'Generate multiple real page versions before this lifecycle test').toBeTruthy()
  const context=await browser.newContext({baseURL:process.env.AGENTX_E2E_BASE_URL,locale:'zh-CN',viewport:{width:1440,height:900}})
  const employee=await context.newPage()
  const result:Record<string,unknown>={passed:false}
  try{
    const token=await loginAt(employee,`/a/${current.slug}`,process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!)
    const headers={Authorization:`Bearer ${token}`}
    const getTask=async(id:string)=>{
      const response=await employee.request.get(`${root}/gateway/v1/tasks/${id}`,{headers})
      expect(response.ok(),await response.text()).toBe(true);return await response.json() as Task
    }
    let frame=await businessFrame(employee)
    await fillFields(frame,scenario.inputs[0])
    const submitted=employee.waitForResponse(response=>response.url().endsWith(`/pages/${current.slug}/invocations`)&&response.request().method()==='POST')
    await frame.locator('[data-action="invoke"]').click()
    const created=await submitted;expect(created.ok(),await created.text()).toBe(true)
    const original=await created.json() as Task
    let pending:Task|undefined
    await expect.poll(async()=>{pending=await getTask(original.taskId);return pending.pendingInputs.length},{timeout:60_000}).toBe(1)
    result.originalTask=pending
    const checkpoint=join(directory,'checkpoint.json')
    const checkpointState=(phase:string)=>{const temporary=checkpoint+'.tmp';writeFileSync(temporary,JSON.stringify({phase,taskId:original.taskId}));renameSync(temporary,checkpoint)}
    checkpointState('waiting_for_upgrade')
    await expect.poll(()=>existsSync(join(directory,'upgrade.json')),{timeout:180_000,intervals:[500,1000]}).toBe(true)
    const upgrade=JSON.parse(readFileSync(join(directory,'upgrade.json'),'utf8')) as {activeBundleId:string}
    expect(upgrade.activeBundleId).not.toBe(original.bundleId)
    expect((await getTask(original.taskId)).bundleId).toBe(original.bundleId)
    expect((await getTask(original.taskId)).pendingInputs.map(row=>row.waitId)).toEqual(pending!.pendingInputs.map(row=>row.waitId))
    result.businessUpgrade=upgrade
    await page.goto(`/applications/${scenario.applicationId}`)
    const editorBound=page.waitForResponse(response=>response.url().endsWith('/builder-sessions')&&response.request().method()==='POST')
    await page.getByRole('tab',{name:'访问界面',exact:true}).click()
    const editorBinding=await editorBound;expect(editorBinding.ok(),await editorBinding.text()).toBe(true)
    const panel=page.getByTestId('application-page-publication')
    const binding=await read<Array<{id:string;htmlHash:string;revision:number}>>(page,admin,`/applications/${scenario.applicationId}/ui/drafts`)
    await panel.getByTestId('application-page-history').click()
    await page.getByTestId(`page-history-${previous.id}`).click()
    await expect(page.getByTestId('application-page-history-preview')).toBeVisible()
    const rollback=page.waitForResponse(response=>response.url().endsWith(`/applications/${scenario.applicationId}/ui/publications`)&&response.request().method()==='POST')
    await page.getByRole('button',{name:'回滚到此记录',exact:true}).click()
    await page.getByRole('button',{name:'确认回滚',exact:true}).click()
    const changed=await rollback;expect(changed.ok(),await changed.text()).toBe(true)
    expect(changed.request().postDataJSON().uiVersionId).toBe(previous.uiVersionId)
    let active:Publication|undefined
    await expect.poll(async()=>{active=(await listing()).publications.find(row=>row.status==='active');return active?.revision},{timeout:120_000}).toBe(initial.revision+1)
    expect(active!.uiVersionId).toBe(previous.uiVersionId)
    expect(active!.manifest.htmlHash).toBe(previous.manifest.htmlHash)
    expect(await read(page,admin,`/applications/${scenario.applicationId}/ui/drafts`)).toEqual(binding)
    const stale=await employee.request.post(`${root}/gateway/v1/pages/${current.slug}/invocations`,{headers:{...headers,'Idempotency-Key':crypto.randomUUID()},data:{publicationId:current.id,revision:current.revision,input:scenario.inputs[1]}})
    expect(stale.status()).toBe(409)
    result.rollback={publication:active,staleSubmissionStatus:stale.status()}
    result.workingPagePreserved=binding
    await page.getByRole('button',{name:'关闭',exact:true}).click()
    // Reload while IAM is available, then continue the original bound page.
    await employee.reload()
    await expect(employee.getByTestId('application-current-task')).toHaveText(original.taskId)
    const view=await employee.request.get(`${root}/gateway/v1/pages/${current.slug}?taskId=${original.taskId}`,{headers})
    expect(view.ok(),await view.text()).toBe(true)
    const frozen=await view.json()
    expect(frozen.historical).toBe(true)
    expect(frozen.publication.publicationId).toBe(current.id)
    expect(frozen.publication.contract.bundleId).toBe(original.bundleId)
    checkpointState('waiting_for_fault')
    await expect.poll(()=>existsSync(join(directory,'fault.json')),{timeout:300_000,intervals:[500,1000]}).toBe(true)
    result.fault=JSON.parse(readFileSync(join(directory,'fault.json'),'utf8'))
    try{
      const unavailable=await employee.request.get('/api/v1/bootstrap/status')
      expect(unavailable.status()).toBeGreaterThanOrEqual(500)
      result.controlUnavailable={kind:'http',status:unavailable.status()}
    }catch(error){
      // A stopped upstream can time out instead of returning HTTP5xx. The
      // Python fault proof independently establishes zero Control replicas.
      expect(error).toBeInstanceOf(errors.TimeoutError)
      result.controlUnavailable={kind:'request_timeout',timeoutMs:20_000}
    }
    pending=await getTask(original.taskId)
    expect(pending.pendingInputs.map(row=>row.waitId)).toEqual((result.originalTask as Task).pendingInputs.map(row=>row.waitId))
    frame=await businessFrame(employee)
    const wait=pending.pendingInputs[0]!,scope=frame.locator(`[data-wait-id="${wait.waitId}"]`)
    await fillFields(scope,scenario.waitInputs![0]![wait.nodePath]![0])
    const sent=employee.waitForResponse(response=>response.url().endsWith(`/invocations/${original.taskId}/waits/${wait.waitId}/input`)&&response.request().method()==='POST')
    await scope.locator('[data-action="submit-input"]').click()
    const accepted=await sent;expect(accepted.ok(),await accepted.text()).toBe(true)
    let final:Task|undefined
    await expect.poll(async()=>{final=await getTask(original.taskId);return final.state},{timeout:90_000}).toBe('completed')
    expect(final!.outputs).toEqual(scenario.expected[0])
    expect(final!.bundleId).toBe(original.bundleId)
    expect(final!.accessBinding).toEqual(original.accessBinding)
    for(const value of Object.values(scenario.expected[0]!))if(typeof value==='string')await expect(frame.locator('body')).toContainText(value)
    await employee.screenshot({path:info.outputPath('completed-with-control-offline.png'),fullPage:true})
    result.final=final;result.passed=true
  }finally{
    writeFileSync(join(directory,'result.json'),JSON.stringify(result,null,2))
    await context.close()
  }
})
