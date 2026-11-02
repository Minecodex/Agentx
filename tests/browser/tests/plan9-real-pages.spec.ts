import { expect, test, type Page } from '@playwright/test'
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import { basename, dirname, join } from 'node:path'
import { allEntries, login, read } from '../studio-builder-support'
import { useRuntimePortForward } from './playground-helpers'
import { trackAccessToken } from '../browser-http'
import { executeCase, verifyHistoricalPage, WorkflowExecutionFailure, type Scenario, type Task } from '../plan9-page-support'
import { addGuidance, type Guidance } from '../plan9-authoring-input'

type Question={questionId:string;question:string;options?:string[]}
type Run = {id:string;status:string;unansweredQuestion?:Question}
type Entry = {runId:string|null;payload:{type:string;reason?:string;code?:string;is_error?:boolean;questionId?:string;question?:string;options?:string[];message?:{isError?:boolean;content?:string}}}
type Result = {scene:string;round:number;draftId?:string;sessionId?:string;attempts:Array<{run:Run;[key:string]:unknown}>;executions:unknown[];modification?:unknown;hostRecovery?:unknown;history?:unknown;elapsedMs?:number;passed:boolean;workflowFailure?:{task:Task;diagnostic:string};guidanceInputs?:Guidance[]}
const scenarios:Scenario[]=process.env.AGENTX_E2E_PLAN9_SCENARIOS?JSON.parse(readFileSync(process.env.AGENTX_E2E_PLAN9_SCENARIOS,'utf8')):[]
const reportPath=process.env.AGENTX_E2E_PLAN9_REPORT!
// Exercise file, wait and approval boundaries before spending time on simpler layouts.
const priority=['S04','S08','S12','S19','S11','S13','S20']
const rank=(scene:string)=>{const index=priority.indexOf(scene.split('-')[0]!);return index<0?priority.length:index}
function persist(reportPath:string,result:Result){
  const directory=join(dirname(reportPath),basename(reportPath,'.json'))
  mkdirSync(directory,{recursive:true})
  const path=join(directory,`${result.scene}-r${result.round}.json`),temporary=`${path}.tmp`
  writeFileSync(temporary,JSON.stringify(result,null,2));renameSync(temporary,path)
}
const requests=[
  '创建清晰易用的桌面业务界面，表单与结果分区，严格按应用契约执行任务并显示真实状态、结果和全部待办。',
  '创建紧凑的桌面业务界面，输入简洁，右侧突出最终业务结果，显示真实任务进度和全部待办。',
  '创建桌面业务界面，增加本人历史和进行中的任务，可重新打开原任务。保留全部业务输入、校验、结果和待办。',
]

function repairDiagnostic(error:string,scenario:Scenario){
  const values:unknown[]=[]
  const paths=new Set<string>()
  const collect=(value:unknown,path='')=>{
    if(path)paths.add(path)
    if(Array.isArray(value))value.forEach((item,index)=>collect(item,`${path}/${index}`))
    else if(value&&typeof value==='object')for(const [key,item]of Object.entries(value))collect(item,`${path}/${key.replaceAll('~','~0').replaceAll('/','~1')}`)
    else if(value!==null&&value!==undefined)values.push(value)
  }
  collect(scenario.inputs);collect(scenario.expected);collect(scenario.waitInputs);collect(scenario.followups)
  for(const input of scenario.inputs)collect(input)
  for(const waits of scenario.waitInputs??[])for(const contents of Object.values(waits))for(const content of contents)collect(content)
  const protectedPaths=[...paths].sort((a,b)=>b.length-a.length).map((path,index)=>({path,marker:`__P9_FIELD_PATH_A${index}__`}))
  for(const {path,marker}of protectedPaths)error=error.replaceAll(path,marker)
  // The held-out business values never enter a generation or repair prompt.
  for(const value of values.sort((a,b)=>String(b).length-String(a).length))for(const spelling of [String(value),typeof value==='number'?value.toLocaleString('en-US'):String(value)]){
    const escaped=spelling.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')
    error=error.replace(new RegExp(`(?<![\\w.])${escaped}(?![\\w.])`,'g'),'[业务验收数据已隐藏]')
  }
  for(const {path,marker}of protectedPaths)error=error.replaceAll(marker,path)
  if(/\(root\)|根|JSON Pointer/.test(error))error+=' 补充输入表单的字段路径相对于该 waitId 的 responseSchema，不是初始 inputSchema。字符串等待使用空 JSON Pointer data-field-path="" 并提交原始字符串；不要给初始对象添加隐藏占位字段，也不要跳过检查。'
  return error.slice(0,10000)
}

function runDiagnostic(run:Run,entries:Entry[]){
  return JSON.stringify({status:run.status,unansweredQuestion:run.unansweredQuestion,events:entries.filter(entry=>entry.runId===run.id).flatMap<{type:string;reason?:string;error?:string}>(({payload})=>{
    if(payload.type==='terminal'||payload.type==='agent_ended')return [{type:payload.type,reason:payload.reason}]
    if(payload.type==='model_failed')return [{type:payload.type,error:payload.code}]
    if(payload.message?.isError)return [{type:payload.type,error:payload.message.content?.slice(0,1800)}]
    return []
  }).slice(-8)})
}

async function waitRun(page:Page,token:ReturnType<typeof trackAccessToken>,session:string,id:string){
  let run:Run|undefined,question:Question|undefined,cursor=0
  const events:Entry[]=[]
  await expect.poll(async()=>{
    const value=await read<{runs:Run[]}>(page,await token(),`/application-ui/sessions/${session}`)
    run=value.runs.find(value=>value.id===id)
    if(run?.status==='waiting'){
      const next=await read<{items:Entry[];nextCursor:number}>(page,await token(),`/design-agent/sessions/${session}/entries?after=${cursor}`)
      events.push(...next.items);cursor=next.nextCursor
      const answers=new Set(events.filter(entry=>entry.payload.type==='answer').map(entry=>entry.payload.questionId))
      const blocked=next.items.length<200?events.find(entry=>entry.runId===id&&entry.payload.type==='question'&&entry.payload.questionId&&!answers.has(entry.payload.questionId)):undefined
      if(blocked)question={questionId:blocked.payload.questionId!,question:blocked.payload.question!,options:blocked.payload.options}
    }
    return Boolean(question||run&&['completed','failed','cancelled','outcome_unknown'].includes(run.status))
  },{timeout:600_000,intervals:[1000,2000]}).toBe(true)
  if(question){
    // A request to bypass a check is a failed attempt, never consent to bypass.
    // Cancel through the real UI and use only the remaining external repair slots.
    const stopped=page.waitForResponse(response=>response.url().endsWith(`/design-agent/sessions/${session}/runs/${id}/cancel`)&&response.request().method()==='POST')
    await Promise.all([stopped,page.getByTestId('application-page-agent').getByRole('button',{name:'停止',exact:true}).click()])
    const receipt=await stopped;expect(receipt.ok(),await receipt.text()).toBe(true)
    await expect.poll(async()=>{
      run=(await read<{runs:Run[]}>(page,await token(),`/application-ui/sessions/${session}`)).runs.find(value=>value.id===id)
      return Boolean(run&&['completed','failed','cancelled','outcome_unknown'].includes(run.status))
    },{timeout:30_000}).toBe(true)
    return {...run!,unansweredQuestion:question}
  }
  return run!
}

scenarios.sort((left,right)=>rank(left.scene)-rank(right.scene))
test.describe.configure({mode:'parallel'})
for(const source of scenarios)test.describe(source.scene,()=>{
test.describe.configure({mode:'default'})
for(let round=0;round<Number(process.env.AGENTX_E2E_PLAN9_ROUNDS??3);round++){
  test(`${source.scene} round ${round+1}: real page Agent and two unseen business executions`,async({page,browser},info)=>{
    test.setTimeout(2_100_000)
    const scenario={...source,...source.roundApplications?.[round]}
    const began=Date.now()
    const result:Result={scene:scenario.scene,round:round+1,attempts:[],executions:[],passed:false}
    const save=()=>persist(reportPath,result)
    save()
    let session:string|undefined,token=()=>Promise.resolve('')
    try{
    const attemptLimit=scenario.creationChain?scenario.pageAttemptBudget!:3
    expect(Number.isInteger(attemptLimit)&&attemptLimit>=1&&attemptLimit<=3,'A creation chain shares at most three Page authoring Runs across all workflow versions').toBe(true)
    await useRuntimePortForward(page)
    token=trackAccessToken(page,await login(page))
    await page.goto(`/applications/${scenario.applicationId}`)
    const bindingResponse=page.waitForResponse(response=>response.url().endsWith('/builder-sessions')&&response.request().method()==='POST',{timeout:60_000})
    await page.getByRole('tab',{name:'访问界面',exact:true}).click()
    const recoveryReceipt=process.env.AGENTX_E2E_PLAN9_HOST_RECOVERY_RECEIPT
    const requirement=requests[round]!+(scenario.scene==='S20'?' 长报告必须使用 SDK subscribeTask 订阅真实任务，持续展示中间消息和最终结果；连接中断后继续原任务。':'')+(recoveryReceipt?' 主标题尚未确定（host_restart 恢复验收）。必须先通过 ask_user 工具询问，question 严格为“请选择恢复后的页面标题”，options 只有“恢复报价页”。等待我回答后再使用该标题生成完整报价页面，保留所有业务输入与结果。':'')
    const started=()=>page.waitForResponse(response=>/\/application-ui\/sessions\/[^/]+\/runs$/.test(new URL(response.url()).pathname)&&response.request().method()==='POST',{timeout:60_000})
    let running:ReturnType<typeof started>
    const bound=await bindingResponse
    expect(bound.ok(),await bound.text()).toBe(true)
    const binding=await bound.json()
    result.draftId=binding.uiDraftId
    await page.getByTestId('application-page-agent').getByRole('textbox',{name:'描述界面调整',exact:true}).fill(requirement)
    running=started()
    await Promise.all([running,page.getByTestId('application-page-agent').getByRole('button',{name:'发送',exact:true}).click()])
    expect(binding.uiDraftId).toBe(result.draftId)
    result.sessionId=binding.id
    session=result.sessionId!
    save()
    if(!session)throw new Error('The page editor did not bind an authoring session')
    const suffix=(await read<{slug?:string}>(page,await token(),`/applications/${scenario.applicationId}/ui/publications`)).slug??`${scenario.slug}-r${round+1}`
    let finalError=''
      for(let attempt=0;attempt<attemptLimit;attempt++){
        const accepted=await running
        expect(accepted.ok(),await accepted.text()).toBe(true)
        expect(new URL(accepted.url()).pathname).toBe(`/api/v1/application-ui/sessions/${session}/runs`)
        const runId=(await accepted.json()).id
        const startedAt=Date.now()
        let guidance:Guidance|undefined
        // Persist the accepted Run before polling or collecting evidence: even
        // an interrupted attempt has consumed one of the three authoring slots.
        const evidence={attempt,aiAttempt:attempt,run:{id:runId,status:'running'} as Run,htmlHash:'',revision:0,elapsedMs:0,entries:[] as Entry[],stage:'generation',failureClass:'',toolErrors:0,error:'',guidance:undefined as Guidance|undefined}
        result.attempts.push(evidence);save()
        if(scenario.steeringTitle){
          const panel=page.getByTestId('application-page-agent')
          guidance=await addGuidance(page,panel,panel.getByLabel('描述界面调整',{exact:true}),session,runId,`补充本次界面要求：页面主标题使用“${scenario.steeringTitle}”。保留全部业务字段、标签、SDK、状态、等待输入、历史与结果，在当前草稿和本次创作中继续。`)
          result.guidanceInputs??=[];result.guidanceInputs.push(guidance);save()
        }
        if(recoveryReceipt&&attempt===0){
          const panel=page.getByTestId('application-page-agent')
          await expect(panel.getByText('请选择恢复后的页面标题',{exact:true})).toBeVisible({timeout:180_000})
          await expect.poll(()=>existsSync(recoveryReceipt),{timeout:180_000,intervals:[500,1000]}).toBe(true)
          result.hostRecovery=JSON.parse(readFileSync(recoveryReceipt,'utf8'));save()
          await expect.poll(async()=>{try{return Boolean((await read<{runs:Run[]}>(page,await token(),`/application-ui/sessions/${session}`)).runs.find(value=>value.id===runId&&value.status==='waiting'))}catch{return false}},{timeout:120_000}).toBe(true)
          const answered=page.waitForResponse(response=>response.url().endsWith(`/design-agent/sessions/${session}/answers`)&&response.request().method()==='POST')
          await Promise.all([answered,panel.getByRole('button',{name:'恢复报价页',exact:true}).click()])
          const answerReceipt=await answered;expect(answerReceipt.ok(),await answerReceipt.text()).toBe(true)
        }
        const run=await waitRun(page,token,session,runId)
        let draft=await read<{html:string;revision:number;htmlHash:string}>(page,await token(),`/applications/${scenario.applicationId}/ui/drafts/${result.draftId}`)
        const entries=await allEntries<Entry>(page,await token(),session)
        writeFileSync(join(dirname(reportPath),`${scenario.scene}-r${round+1}-attempt${attempt}.html`),draft.html)
        Object.assign(evidence,{run,htmlHash:draft.htmlHash,revision:draft.revision,elapsedMs:Date.now()-startedAt,entries,toolErrors:entries.filter(entry=>entry.runId===run.id&&entry.payload.type==='tool_settled'&&entry.payload.is_error).length,guidance});save()
        let uncertain=run.status==='outcome_unknown'
        try{
          expect(run.status,runDiagnostic(run,entries)).toBe('completed')
          expect(draft.html.trim()).not.toBe('')
          if(scenario.scene==='S01'&&round===0&&!result.modification){
            evidence.stage='natural_language_modification'
            const previous=draft,changedAt=Date.now()
            const requirement='请调整当前界面：主标题改为“客户报价服务台”，输入区放左侧，右侧突出正式报价结果；保留原有字段、标签、稳定标记、SDK 提交、状态和错误处理。请修改现有草稿，不新建界面，并完成检查。'
            await page.getByLabel('描述界面调整',{exact:true}).fill(requirement)
            const modifying=started()
            await Promise.all([modifying,page.getByTestId('application-page-agent').getByRole('button',{name:'发送',exact:true}).click()])
            const acceptedChange=await modifying
            expect(acceptedChange.ok(),await acceptedChange.text()).toBe(true)
            const changedRun=await waitRun(page,token,session,(await acceptedChange.json()).id)
            uncertain=changedRun.status==='outcome_unknown'
            draft=await read(page,await token(),`/applications/${scenario.applicationId}/ui/drafts/${result.draftId}`)
            const changedEntries=await allEntries<Entry>(page,await token(),session)
            result.modification={requirement,run:changedRun,previousHash:previous.htmlHash,htmlHash:draft.htmlHash,elapsedMs:Date.now()-changedAt,entries:changedEntries,toolErrors:changedEntries.filter(entry=>entry.runId===changedRun.id&&entry.payload.type==='tool_settled'&&entry.payload.is_error).length}
            writeFileSync(join(dirname(reportPath),'S01-r1-modified.html'),draft.html);save()
            expect(changedRun.status,runDiagnostic(changedRun,changedEntries)).toBe('completed')
            expect(draft.htmlHash).not.toBe(previous.htmlHash)
            expect(draft.html).toContain('客户报价服务台')
          }
          if(scenario.scene==='S01'&&round===0){
            expect(draft.html).toContain('客户报价服务台')
            if(attempt>0&&result.modification)Object.assign(result.modification,{finalHtmlHash:draft.htmlHash,resolvedByRunId:run.id,finalTitleValidated:true})
          }
          evidence.stage='preview_check'
          await page.getByRole('tab',{name:'检查结果',exact:true}).click()
          const checked=page.waitForResponse(response=>response.url().endsWith(`/ui/drafts/${result.draftId}/checks`)&&response.request().method()==='POST'&&response.request().postDataJSON().mode==='preview')
          await Promise.all([checked,page.getByRole('button',{name:'重新检查',exact:true}).click()])
          const receipt=await checked
          expect(receipt.ok(),await receipt.text()).toBe(true)
          const checks=receipt.request().postDataJSON()
          expect(checks.errors,JSON.stringify(checks.errors)).toEqual([])
          evidence.stage='publication'
          const publishing=page.waitForResponse(response=>response.url().endsWith(`/applications/${scenario.applicationId}/ui/publications`)&&response.request().method()==='POST')
          await page.getByTestId('application-publish-page').click()
          const address=page.getByRole('dialog',{name:'设置访问地址'})
          if(await address.count()){
            await address.getByLabel('访问后缀',{exact:true}).fill(suffix)
            await address.getByRole('button',{name:'发布',exact:true}).click()
          }
          const publication=await publishing
          expect(publication.ok(),await publication.text()).toBe(true)
          const publicationId=(await publication.json()).id as string
          let current:{id:string;manifest:{htmlHash:string};status:string}|undefined
          await expect.poll(async()=>{
            const published=await read<{publications:Array<{id:string;manifest:{htmlHash:string};status:string}>}>(page,await token(),`/applications/${scenario.applicationId}/ui/publications`)
            current=published.publications.find(value=>value.id===publicationId)
            return current?.status
          },{timeout:120_000,intervals:[500,1000]}).toBe('active')
          expect(current!.manifest.htmlHash).toBe(draft.htmlHash)
          await expect(page.getByTestId('application-open-page')).toHaveAttribute('href',`/a/${suffix}`,{timeout:120_000})
          writeFileSync(join(dirname(reportPath),`${scenario.scene}-r${round+1}-attempt${attempt}-publication.json`),JSON.stringify(current,null,2))
          result.executions=[]
          evidence.stage='business_execution'
          for(let index=0;index<scenario.inputs.length;index++){
            result.executions.push(await executeCase(browser,scenario,suffix,index,info,attempt));save()
          }
          // A second explicit publication exercises the stable URL and leaves
          // genuine original task bindings available for history/recovery tests.
          const republishing=page.waitForResponse(response=>response.url().endsWith(`/applications/${scenario.applicationId}/ui/publications`)&&response.request().method()==='POST')
          await page.getByTestId('application-publish-page').click()
          const reissued=await republishing;expect(reissued.ok(),await reissued.text()).toBe(true)
          const reissuedId=(await reissued.json()).id
          await expect.poll(async()=>(await read<{activePublicationId:string}>(page,await token(),`/applications/${scenario.applicationId}/ui/publications`)).activePublicationId,{timeout:120_000}).toBe(reissuedId)
          if(round===2){evidence.stage='historical_version';result.history=await verifyHistoricalPage(browser,scenario,suffix,info)}
          result.passed=true;save();break
        }catch(error){
          finalError=String(error);evidence.error=finalError;evidence.failureClass=uncertain?'result_unknown':evidence.stage;save()
          if(error instanceof WorkflowExecutionFailure){
            result.workflowFailure={task:error.task,diagnostic:error.diagnostic};evidence.failureClass='workflow_execution';save()
            await page.screenshot({path:info.outputPath(`generation-attempt${attempt}-failure.png`),fullPage:true})
            break
          }
          await page.screenshot({path:info.outputPath(`generation-attempt${attempt}-failure.png`),fullPage:true})
          if(attempt===attemptLimit-1||finalError.includes('PLAN9_BUSINESS_OUTCOME_UNKNOWN'))break
          // A fresh authorized design Run can repair the saved draft. It must
          // never replay the unknown Effect or retry an unverified business write.
          const recovery=uncertain?' 上一创作 Run 的结果未知，保留其状态和费用；这是新的草稿修复请求。先读取当前已保存页面及修订，不重放旧 Effect 或旧编辑命令。':''
          const plannedModification=scenario.scene==='S01'&&round===0?' 主标题必须为“客户报价服务台”，输入在左、正式报价结果在右。':''
          await page.getByLabel('描述界面调整',{exact:true}).fill(`验收失败，请通过页面工具修复当前界面。诊断：${repairDiagnostic(finalError,scenario)}。必须保留原业务契约，运行静态检查和交互模拟。只使用 SDK 获取真实业务结果，不在网页计算正式结果或伪造成功。${recovery}${plannedModification}`)
          running=started()
          await Promise.all([running,page.getByTestId('application-page-agent').getByRole('button',{name:'发送',exact:true}).click()])
        }
      }
      expect(result.passed,finalError).toBe(true)
    }finally{
      result.elapsedMs=Date.now()-began;save()
      if(session){
        const runs=await read<{runs:Run[]}>(page,await token(),`/application-ui/sessions/${session}`)
        for(const run of runs.runs)if(['queued','running','waiting'].includes(run.status))await page.request.post(`/api/v1/design-agent/sessions/${session}/runs/${run.id}/cancel`,{headers:{Authorization:`Bearer ${await token()}`}})
      }
      await info.attach('page-acceptance',{body:JSON.stringify(result),contentType:'application/json'})
    }
  })
}
})
