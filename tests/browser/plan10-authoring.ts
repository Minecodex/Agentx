import { expect, type Page } from '@playwright/test'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { allEntries, login, read } from './studio-builder-support'
import { trackAccessToken } from './browser-http'
import { useRuntimePortForward } from './tests/playground-helpers'

export type BatchScenario={scene:string;kind:string;applicationId:string;slug:string;inputs:Record<string,unknown>[];expected:Record<string,unknown>[];workflowId:string;deploymentId:string}
type Run={id:string;status:string}
type Draft={html:string;revision:number;htmlHash:string}
type Binding={id:string;uiDraftId:string}
type Token=ReturnType<typeof trackAccessToken>

const directory=process.env.AGENTX_E2E_PLAN10_DIRECTORY!
export function evidence(name:string,value:unknown){mkdirSync(directory,{recursive:true});writeFileSync(join(directory,`${name}.json`),JSON.stringify(value,null,2))}
function requireModelBudget(){
  const {requests,maxRequests}=JSON.parse(readFileSync(process.env.AGENTX_E2E_PLAN10_BUDGET_FILE!,'utf8')) as {requests:number;maxRequests:number}
  if(requests>=maxRequests)throw new Error(`Plan10 model request limit reached (${requests}/${maxRequests}); no new authoring run was submitted`)
}

async function waitRun(page:Page,token:Token,session:string,id:string){
  let run:Run|undefined
  await expect.poll(async()=>{
    run=(await read<{runs:Run[]}>(page,await token(),`/application-ui/sessions/${session}`)).runs.find(run=>run.id===id)
    return run?.status
  },{timeout:660_000,intervals:[1000,2000]}).toMatch(/^(completed|failed|cancelled|outcome_unknown)$/)
  return run!
}

export async function author(page:Page,scenario:BatchScenario){
  await useRuntimePortForward(page)
  const token=trackAccessToken(page,await login(page))
  await page.goto(`/applications/${scenario.applicationId}`)
  const bindingReceipt=page.waitForResponse(response=>response.url().endsWith('/builder-sessions')&&response.request().method()==='POST')
  await page.getByRole('tab',{name:'访问界面',exact:true}).click()
  const binding=await (await bindingReceipt).json() as Binding
  const panel=page.getByTestId('application-page-agent')
  const attempts:unknown[]=[]
  const requirement='请创建这个应用的桌面业务页面：保留单条办理，增加批量办理和本人记录。根据宿主 context.locale 提供中文和英文操作及字段，遵守 context.theme 的明暗主题。批量办理支持添加、删除和逐行编辑。'+(scenario.kind==='quote'?'CSV/TSV 文件或粘贴导入，显式选择列映射并确认预览后才能提交。':'')+'一次提交创建一个持久批次和全部完整输入行。提交后可关页重开原批次、分页查看真实进度、处理原任务的补充资料和审批入口，支持停止派发、取消、仅对允许的失败项派生重做和导出 CSV。导出按钮标记 data-action="export-batch"，生成完成后的 CSV 下载按钮标记 data-action="download-batch-export"，下载必须调用 SDK。刷新不能清空正在编辑的输入。金额区分币种。使用已有 SDK2 和页面检查标记，自己读取应用契约并完成模拟检查后交付。不需要询问标题，使用应用的业务名称。'
  let diagnostic='',draft:Draft|undefined
  const priorDiagnostics=JSON.parse(process.env.AGENTX_E2E_PLAN10_DIAGNOSTICS??'{}') as Record<string,Array<{code:string;message:string}>>
  const focus:Record<string,string>={
    expense:'上轮已修好 waitingInput/waitingApproval/outcomeUnknown 的计数映射，真实页面已经正确显示“等待补充: 3”，不要重做这部分。仅处理两处剩余显示错误：(1) STKEY 中误加了 accepted 和 finished 两个不存在的计数字段，删除这两个元组，保留其他现有映射；不要创建任何新计数字段或词典键。(2) 读取 function renderItems 的完整原文，只把其中直接显示 it.dispatchState 的文字改用已有 d_* 词典，实际值不变。批次标题派发文字已本地化，不要再改。完成后跑 static 和 preview，其他功能、布局、行标识、词典保持原样。',
    quote:'当前类型、批量入口、单项及两批正式报价/导入/导出业务已通过，不改这些逻辑。只修最新视觉问题：英文导入文件和分隔符标签仍有中文；btnBackSingle 是无文字的空白按钮，与页头入口重复则移除及无用绑定，否则补充准确本地化名称。结果表 renderItems 的状态列表头误用 T.counts.total，要改为状态；原状态/派发码用中英文可读标签展示，数据不变。现有行点击打开详情的逻辑保留，但行内增加有明确名称的原生查看详情按钮，让键盘 Enter 也能打开原任务详情。',
    file:'只需两个小补丁，不再设计或重构：(1) renderBatch 用 el.innerHTML 重建区域时，会清空 itemsHolder 和 itemTaskHolder。采用保留现有 DOM 节点的最小方式：赋值前记住这两个原节点；赋值后以原节点替换新建的空占位节点；原节点不存在的首次渲染保留新占位。这样轮询和导出刷新只更新摘要，原明细、已选任务内容和事件处理都保留。不改 pollBatch 或其他业务。(2) renderItems 的 clientItemId 表头使用已有 T.rowLabel，数据 ID 不变。先 page_read 读取这两个函数的完整原文与最新 metadata，再用一次精确编辑完成这两处；不能沿用旧 revision。语法、stTag、文件/SHA/导出均已通过，不修改。直接调用工具，不整页重写，不输出长篇设计。编辑 ACK 后用最新哈希在同一轮请求 static 与 preview 检查，均通过后结束。',
  }
  const sourceHints=()=>draft?['var STKEY','const STKEY','function stateTag','function stTag','function countsHtml','function renderBatch','function renderItems','function renderRows','function setLang','function openBatchView','btnBackSingle','importFile','importDelim','clientItemId'].flatMap(term=>{
    const index=draft!.html.indexOf(term)
    return index<0?[]:[{term,offset:[...draft!.html.slice(0,index)].length}]
  }):[]
  const initialRequest=()=>process.env.AGENTX_E2E_PLAN10_RESUME==='1'
    ?`请继续修复已有 AI 工作 HTML，保留当前业务功能、布局和 SDK，不重新设计或整页重写。本轮只定点处理实际错误：${focus[scenario.kind]} 最近宿主检查诊断（无真实业务输入）：${JSON.stringify(priorDiagnostics[scenario.kind]??[])}。现有 Batch.state 的值为 running、waiting、partial_failure、cancelled、completed、outcome_unknown；计数属性为 total/queued/running/waitingInput/waitingApproval/completed/failed/cancelled/outcomeUnknown/ended；只本地化展示，不改真实键和值。当前原始源码的相关字符偏移提示：${JSON.stringify(sourceHints())}。page_read 的 offset/limit 是字符数，不是行数；读取相关函数的完整定义，合并相邻区域，避免逐几十字符读取或反复读取无关区域。page_edit 的 oldText/newText 是真实源码字符串，JSON 只转义一次；使用当前原文中唯一的完整片段。完成必要编辑后使用 ACK 的最新哈希，可在同一轮请求 page_read 和 static、preview 检查；通过后直接结束，不扩展功能或反复解释。`
    :requirement
  const send=async(text:string)=>{
    requireModelBudget()
    await panel.getByRole('textbox',{name:'描述界面调整',exact:true}).fill(text)
    const response=page.waitForResponse(response=>response.url().endsWith(`/application-ui/sessions/${binding.id}/runs`)&&response.request().method()==='POST')
    await panel.getByRole('button',{name:'发送',exact:true}).click()
    const accepted=await response;expect(accepted.ok(),await accepted.text()).toBe(true)
    return (await accepted.json() as Run).id
  }
  let retainedPassed=false
  if(process.env.AGENTX_E2E_PLAN10_RESUME==='1'){
    draft=await read<Draft>(page,await token(),`/applications/${scenario.applicationId}/ui/drafts/${binding.uiDraftId}`)
    if(draft.html.trim()){
      await page.getByRole('tab',{name:'检查结果',exact:true}).click()
      const result=(mode:string)=>page.waitForResponse(response=>response.url().endsWith(`/ui/drafts/${binding.uiDraftId}/checks`)&&response.request().method()==='POST'&&response.request().postDataJSON().mode===mode)
      const [staticCheck,previewCheck]=await Promise.all([result('static'),result('preview'),page.getByRole('button',{name:'重新检查',exact:true}).click()])
      expect(staticCheck.ok(),await staticCheck.text()).toBe(true);expect(previewCheck.ok(),await previewCheck.text()).toBe(true)
      const reports=[staticCheck.request().postDataJSON(),previewCheck.request().postDataJSON()]
      const errors=reports.flatMap(report=>report.errors as Array<{code:string;message:string}>)
      const reviewPath=join(directory,`${scenario.kind}-visual-review.json`)
      if(existsSync(reviewPath)){
        const review=JSON.parse(readFileSync(reviewPath,'utf8')) as {htmlHash:string;passed:boolean;issues:string[]}
        if(review.htmlHash===draft.htmlHash&&!review.passed)errors.push({code:'PAGE_DESKTOP_LOCALIZATION',message:review.issues.join('; ')})
      }
      priorDiagnostics[scenario.kind]=errors
      retainedPassed=errors.length===0&&draft.html.includes('create-batch')
      evidence(`${scenario.kind}-retained-preflight`,{binding,draft,reports,errors,passed:retainedPassed,entries:await allEntries(page,await token(),binding.id)})
      writeFileSync(join(directory,`${scenario.kind}-retained-preflight.html`),draft.html)
    }
  }
  for(let attempt=0;!retainedPassed&&attempt<3;attempt++){
    const id=await send(attempt?`请修复当前工作页面并重新检查，保留业务字段与批量功能。检查诊断：${diagnostic.slice(0,8000)}`:initialRequest())
    const record:{id:string;status:string;draft?:Draft;entries?:unknown;error?:string}={id,status:'running'}
    attempts.push(record);evidence(`${scenario.kind}-authoring`,{requirement,binding,attempts})
    let run:Run|undefined,modelFailure:string|undefined
    try{
      run=await waitRun(page,token,binding.id,id)
      draft=await read<Draft>(page,await token(),`/applications/${scenario.applicationId}/ui/drafts/${binding.uiDraftId}`)
      record.status=run.status;record.draft=draft;record.entries=await allEntries(page,await token(),binding.id)
      writeFileSync(join(directory,`${scenario.kind}-attempt-${attempt}.html`),draft.html)
      const events=record.entries as Array<{runId:string;payload:{type:string;code?:string}}>
      modelFailure=events.find(event=>event.runId===id&&event.payload.type==='model_failed')?.payload.code
      if(modelFailure)throw new Error(`AI run ${run.status}: ${modelFailure}; inspect the saved trajectory before another model attempt`)
      expect(run.status).toBe('completed');expect(draft.html.trim()).not.toBe('')
      await page.getByRole('tab',{name:'检查结果',exact:true}).click()
      const checking=page.waitForResponse(response=>response.url().endsWith(`/ui/drafts/${binding.uiDraftId}/checks`)&&response.request().method()==='POST'&&response.request().postDataJSON().mode==='preview')
      await page.getByRole('button',{name:'重新检查',exact:true}).click()
      const checked=await checking;expect(checked.ok(),await checked.text()).toBe(true)
      expect(checked.request().postDataJSON().errors).toEqual([])
      expect(draft.html).toContain('create-batch')
      diagnostic='';evidence(`${scenario.kind}-authoring`,{requirement,binding,attempts});break
    }catch(error){
      diagnostic=error instanceof Error?error.message:String(error);record.error=diagnostic
      if(!record.draft){
        draft=await read<Draft>(page,await token(),`/applications/${scenario.applicationId}/ui/drafts/${binding.uiDraftId}`)
        record.draft=draft;record.entries=await allEntries(page,await token(),binding.id)
        writeFileSync(join(directory,`${scenario.kind}-attempt-${attempt}.html`),draft.html)
      }
      evidence(`${scenario.kind}-authoring`,{requirement,binding,attempts})
      if(modelFailure||!run||run.status==='outcome_unknown'||attempt===2)throw error
    }
  }
  expect(diagnostic).toBe('')
  const suffix=`${scenario.slug}-batch`
  const publishing=page.waitForResponse(response=>response.url().endsWith(`/applications/${scenario.applicationId}/ui/publications`)&&response.request().method()==='POST')
  await page.getByTestId('application-publish-page').click()
  const address=page.getByRole('dialog',{name:'设置访问地址'})
  if(await address.count()){
    await address.getByLabel('访问后缀',{exact:true}).fill(suffix)
    await address.getByRole('button',{name:'发布',exact:true}).click()
  }
  const receipt=await publishing;expect(receipt.ok(),await receipt.text()).toBe(true)
  const publication=await receipt.json() as {id:string}
  await expect.poll(async()=>(await read<{activePublicationId:string}>(page,await token(),`/applications/${scenario.applicationId}/ui/publications`)).activePublicationId,{timeout:120_000}).toBe(publication.id)
  evidence(`${scenario.kind}-publication`,{publication,suffix,htmlHash:draft!.htmlHash,binding})
  return{suffix,publicationId:publication.id,token,binding,draft:draft!}
}

/** Repair the same working HTML through the original Agent; held-out values stay out of prompts. */
export async function repairPage(page:Page,scenario:BatchScenario,previous:Awaited<ReturnType<typeof author>>,failure:unknown,attempt:number){
  requireModelBudget()
  const message=failure instanceof Error?failure.message:String(failure)
  const selectors=[...new Set(message.match(/\[data-[^\]]+\]/g)??[])].slice(0,12)
  const labelHint=message.includes('SDK property names')?'状态徽标和进度计数应显示本地化标签，不能把 waiting_input、waiting_approval、outcome_unknown 直接作为 UI 文案；clientItemId 表头改为本地化的业务行标识，原状态值和实际 ID 不变。':''
  const diagnosis=`正式桌面验收遇到了界面问题。请检查并修复当前工作 HTML，保留单项、批量、本人记录、真实输出和等待处理。关注这些控件：${selectors.join('；')||'批次恢复、结果展示、导入列映射和导出按钮'}。${labelHint}不要猜测或硬编码测试业务值。根据 context.locale 切换中英文操作和字段；批量行内的行标题、字段标签、删除按钮也必须切换，不能只翻译页头。遵守 context.theme 明暗主题；轮询和网络错误保留编辑。导入需保留显式列映射、数值约束和编辑预览，使用 SDK 文档的标记与 readTable。导出和 CSV 下载按钮分别标记 data-action="export-batch" 与 data-action="download-batch-export"，下载走 SDK。通过模拟检查。`
  const panel=page.getByTestId('application-page-agent')
  await panel.getByRole('textbox',{name:'描述界面调整',exact:true}).fill(diagnosis)
  const running=page.waitForResponse(response=>response.url().endsWith(`/application-ui/sessions/${previous.binding.id}/runs`)&&response.request().method()==='POST')
  await panel.getByRole('button',{name:'发送',exact:true}).click()
  const receipt=await running;expect(receipt.ok(),await receipt.text()).toBe(true)
  const id=(await receipt.json() as Run).id
  evidence(`${scenario.kind}-repair-${attempt}`,{diagnosis,originalFailure:message,run:{id,status:'running'},binding:previous.binding})
  const run=await waitRun(page,previous.token,previous.binding.id,id)
  const draft=await read<Draft>(page,await previous.token(),`/applications/${scenario.applicationId}/ui/drafts/${previous.binding.uiDraftId}`)
  const entries=await allEntries(page,await previous.token(),previous.binding.id)
  evidence(`${scenario.kind}-repair-${attempt}`,{diagnosis,originalFailure:message,run,draft,entries,previousHash:previous.draft.htmlHash})
  writeFileSync(join(directory,`${scenario.kind}-repair-${attempt}.html`),draft.html)
  expect(run.status).toBe('completed')
  await page.getByRole('tab',{name:'检查结果',exact:true}).click()
  const checking=page.waitForResponse(response=>response.url().endsWith(`/ui/drafts/${previous.binding.uiDraftId}/checks`)&&response.request().method()==='POST'&&response.request().postDataJSON().mode==='preview')
  await page.getByRole('button',{name:'重新检查',exact:true}).click()
  const checked=await checking;expect(checked.ok(),await checked.text()).toBe(true)
  expect(checked.request().postDataJSON().errors).toEqual([])
  const publishing=page.waitForResponse(response=>response.url().endsWith(`/applications/${scenario.applicationId}/ui/publications`)&&response.request().method()==='POST')
  await page.getByTestId('application-publish-page').click()
  const published=await publishing;expect(published.ok(),await published.text()).toBe(true)
  const publication=await published.json() as {id:string}
  await expect.poll(async()=>(await read<{activePublicationId:string}>(page,await previous.token(),`/applications/${scenario.applicationId}/ui/publications`)).activePublicationId,{timeout:120_000}).toBe(publication.id)
  return{...previous,draft,publicationId:publication.id}
}
