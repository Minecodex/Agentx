import {expect} from '@playwright/test'
import {readFileSync} from 'node:fs'
import {trackAccessToken} from '../browser-http'
import {test,allEntries,completed,count,create,editorPath,fixtureCanvas,login,read,saved,send,setModel,trackDesignRun} from '../studio-builder-support'
import {useRuntimePortForward} from './playground-helpers'

type Entry={runId:string;payload:{type:string;message?:{role:string;content:string};command?:{document:unknown}}}
const configuration=JSON.parse(readFileSync(process.env.AGENTX_E2E_DESIGN_BROWSER_STATE!,'utf8')) as {models:Record<string,{alias:string}>;application:{applicationId:string}}
const live=process.env.AGENTX_E2E_STUDIO_LIVE_ALIAS

function observations(entries:Entry[],runId:string){
  return entries.filter(entry=>entry.runId===runId&&entry.payload.message?.role==='tool_result').flatMap(entry=>{
    try{const value=JSON.parse(entry.payload.message!.content) as {source?:string;documentHash:string;image?:{artifactId:string;sizeBytes:number};report:{errors:string[];actions:{action?:string;status?:string}[];dom:{text:string};facts?:{counts?:{invoke:number;formalRequests:number};latestTask?:{taskId:string;state:string}}}};return value.source==='isolated_browser'?[value]:[]}catch{return[]}
  })
}

test('Fixture: real browser images return to the same Agent loop and layout edits preserve the Definition',async({page},info)=>{
  const {token,created}=await fixtureCanvas(page,'Browser geometry')
  await send(page,'P8FIX: whole with_loop label=Browser geometry')
  await completed(page)
  const before=await saved(page,token,created)
  const accepted=page.waitForResponse(response=>/\/design-agent\/sessions\/.+\/runs$/.test(response.url())&&response.request().method()==='POST')
  const runId=await send(page,'P8FIX: browser vision move recover_generation')
  const session=new URL((await accepted).url()).pathname.split('/').at(-2)!
  await completed(page)
  const after=await saved(page,token,created)
  expect(after.definition).toEqual(before.definition)
  const entries=await allEntries<Entry>(page,token,session)
  const reports=observations(entries,runId)
  expect(entries.filter(entry=>entry.runId===runId&&entry.payload.message?.content.includes('A confirmed model generation failed;'))).toHaveLength(2)
  expect(reports).toHaveLength(2)
  expect(reports.map(report=>report.report.errors)).toEqual([[],[]])
  expect(reports.every(report=>Boolean(report.image)&&report.image!.sizeBytes<576*1024)).toBe(true)
  await expect(page.getByTestId('studio-builder-panel')).toContainText('images=2; observations=2; moved=true')
  await expect(page.getByTestId('design-browser-observation')).toHaveCount(2)
  expect(await count(page,token,created.id)).toBe(0)
  await page.reload()
  await page.getByRole('button',{name:'AI 创作',exact:true}).click()
  await expect(page.getByTestId('design-browser-observation')).toHaveCount(2)
  await info.attach('browser-observations.json',{body:JSON.stringify(reports),contentType:'application/json'})
  await setModel(page,configuration.models.text!.alias)
  await page.goto(editorPath(created))
  await page.getByRole('button',{name:'AI 创作',exact:true}).click()
  await send(page,'P8FIX: browser vision denied after model switch')
  await completed(page,false)
  await expect(page.getByTestId('studio-builder-panel')).toContainText('images=0; observations=1; moved=false; vision_denied')
})

test('Fixture: text models receive an explicit image capability rejection',async({page})=>{
  const token=await login(page)
  await setModel(page,configuration.models.text!.alias)
  const created=await create(page,'Browser unsupported image')
  await page.getByRole('button',{name:'AI 创作',exact:true}).click()
  await send(page,'P8FIX: browser vision denied')
  await completed(page,false)
  await expect(page.getByTestId('studio-builder-panel')).toContainText('images=0; observations=1; moved=false; vision_denied')
  const accepted=page.waitForResponse(response=>/\/design-agent\/sessions\/.+\/runs$/.test(response.url())&&response.request().method()==='POST')
  const run=await send(page,'P8FIX: browser dom')
  const session=new URL((await accepted).url()).pathname.split('/').at(-2)!
  await completed(page,false)
  const reports=observations(await allEntries<Entry>(page,token,session),run)
  expect(reports).toHaveLength(1)
  expect(reports[0]!.image).toBeNull()
  expect(reports[0]!.report.errors).toEqual([])
  expect(await count(page,token,created.id)).toBe(0)
})

test('Fixture: actual isolated page observations preserve the opaque frame without injected browser errors',async({page},info)=>{
  const token=await login(page)
  await setModel(page,configuration.models.vision!.alias)
  await page.goto(`/applications/${configuration.application.applicationId}`)
  const accepted=page.waitForResponse(response=>response.url().endsWith('/builder-sessions')&&response.request().method()==='POST')
  await page.getByRole('tab',{name:'访问界面',exact:true}).click()
  const binding=await (await accepted).json() as {id:string}
  const panel=page.getByTestId('application-page-agent')
  await panel.getByRole('textbox',{name:'描述界面调整',exact:true}).fill('P8FIX: page browser')
  const response=page.waitForResponse(response=>response.url().endsWith(`/application-ui/sessions/${binding.id}/runs`)&&response.request().method()==='POST')
  await panel.getByRole('button',{name:'发送',exact:true}).click()
  const submitted=await response
  const run=await submitted.json() as {id:string}
  trackDesignRun(page,run.id,binding.id,submitted.request().headers()['authorization']!)
  await expect.poll(async()=>{
    const state=await read<{runs:Array<{id:string;status:string}>}>(page,token,`/application-ui/sessions/${binding.id}`)
    return state.runs.find(value=>value.id===run.id)?.status
  },{timeout:120_000,intervals:[1000]}).toBe('completed')
  const reports=observations(await allEntries<Entry>(page,token,binding.id),run.id)
  expect(reports).toHaveLength(1)
  expect(reports[0]!.report.errors).toEqual([])
  expect(reports[0]!.image).toBeTruthy()
  await expect(panel).toContainText('P8 page browser evidence: images=1')
  await info.attach('opaque-page-browser.json',{body:JSON.stringify(reports),contentType:'application/json'})
})

test('Real: Workflow creation and visual layout editing finish without a user repair prompt',async({page},info)=>{
  test.skip(!live,'Explicit live test connection is required')
  const token=await login(page)
  const currentToken=trackAccessToken(page,token)
  await setModel(page,live!)
  const created=await create(page,'Vision authored workflow')
  await page.getByRole('button',{name:'AI 创作',exact:true}).click()
  let session=''
  page.on('response',response=>{if(/\/design-agent\/sessions\/.+\/runs$/.test(response.url())&&response.request().method()==='POST')session=new URL(response.url()).pathname.split('/').at(-2)!})
  const createRun=await send(page,'创建一个最简单的客户回显工作流：输入 customer 必填字符串，保留初始受保护的 Exit 并通过正确 InputBinding 输出 customer 字符串。设置正确的开始连线和结束输出契约，不添加其他业务节点或资源。自己静态检查，使用 workflow_browser 截图观察实际画布（screenshot=true，中文浅色，可 focus_nodes 空列表），看完再结束。本轮一次自主完成，不执行工作流或发布。')
  await completed(page)
  const before=await saved(page,token,created)
  const editRun=await send(page,'只调整这个工作流的布局：先用 workflow_browser 截图看实际画布，再把 Exit 节点移到 x=680,y=260，使用 move_nodes 保持全部 Definition、节点配置和连线原样。编辑应用后以当前哈希再截图观察（英文深色），确认位置调整完成再结束。自己处理常规工具错误，不需要再次问我。')
  await completed(page)
  const after=await saved(page,token,created)
  expect(after.definition).toEqual(before.definition)
  expect(after.definition).toMatchObject({start:{inputs:{properties:{customer:{type:'string'}}}}})
  const document=await read<{definition:unknown;editorDocument:{nodeLayouts:Array<{nodeId:string;x:number;y:number}>}}>(page,token,`/workflows/${created.id}/drafts/${created.documentId}`)
  const exitId=after.definition.nodes.find(node=>node.type==='exit')!.id
  expect(document.editorDocument.nodeLayouts.find(node=>node.nodeId===exitId)).toMatchObject({x:680,y:260})
  const validation=await page.request.post(`/api/v1/workflows/${created.id}/drafts/${created.documentId}/validate`,{headers:{Authorization:`Bearer ${await currentToken()}`},data:{definition:document.definition,editorDocument:document.editorDocument}})
  expect(validation.ok(),await validation.text()).toBe(true)
  const checked=await validation.json() as {issues:Array<{severity:string}>}
  expect(checked.issues.filter(issue=>issue.severity==='error')).toEqual([])
  const entries=await allEntries<Entry>(page,token,session)
  expect(observations(entries,createRun).some(report=>report.image)).toBe(true)
  expect(observations(entries,editRun).filter(report=>report.image).length).toBeGreaterThanOrEqual(2)
  expect(observations(entries,editRun).at(-1)!.report.errors).toEqual([])
  expect(await count(page,token,created.id)).toBe(0)
  await info.attach('workflow-design-events.json',{body:JSON.stringify(entries),contentType:'application/json'})
  await page.screenshot({path:info.outputPath('workflow-authored.png')})
})

test('Real: business page creation and modification use browser feedback in one run each',async({page},info)=>{
  test.setTimeout(info.timeout+1_320_000)
  test.skip(!live,'Explicit live test connection is required')
  const token=await login(page)
  await useRuntimePortForward(page)
  await setModel(page,live!)
  const application=configuration.application.applicationId
  await page.goto(`/applications/${application}`)
  const response=page.waitForResponse(response=>response.url().endsWith('/builder-sessions')&&response.request().method()==='POST')
  await page.getByRole('tab',{name:'访问界面',exact:true}).click()
  const binding=await (await response).json() as {id:string;uiDraftId:string}
  const panel=page.getByTestId('application-page-agent')
  const submit=async(prompt:string)=>{
    await panel.getByRole('textbox',{name:'描述界面调整',exact:true}).fill(prompt)
    const accepted=page.waitForResponse(response=>response.url().endsWith(`/application-ui/sessions/${binding.id}/runs`)&&response.request().method()==='POST')
    await panel.getByRole('button',{name:'发送',exact:true}).click()
    const result=await accepted;expect(result.ok(),await result.text()).toBeTruthy()
    const run=await result.json() as {id:string}
    trackDesignRun(page,run.id,binding.id,result.request().headers()['authorization']!)
    await expect.poll(async()=>{
      const session=await read<{runs:Array<{id:string;status:string}>}>(page,token,`/application-ui/sessions/${binding.id}`)
      return session.runs.find(value=>value.id===run.id)?.status
    },{timeout:660_000,intervals:[1000,2000]}).toMatch(/^(completed|failed|cancelled|outcome_unknown)$/)
    const state=await read<{runs:Array<{id:string;status:string}>}>(page,token,`/application-ui/sessions/${binding.id}`)
    if(state.runs.find(value=>value.id===run.id)?.status!=='completed')await info.attach(`page-run-${run.id}.json`,{body:JSON.stringify(await allEntries<Entry>(page,token,binding.id)),contentType:'application/json'})
    expect(state.runs.find(value=>value.id===run.id)?.status).toBe('completed')
    return run.id
  }
  const created=await submit('创建简洁完整的客户回显桌面业务页，只包含单条办理和本人历史，不增加批量。标题使用“客户回显”。按真实契约和 SDK2 实现提交、订阅进度、显示 customer 结果和原任务刷新恢复，保留检查标记。请自己调用 page_check 静态与模拟预览，使用 page_browser 的真实浏览器操作、screenshot=true 检查布局和交互，再完成交付。自己修复发现的问题，不等我发送继续。')
  const edited=await submit('保留当前完整业务功能，只把标题改为“客户服务台”并优化桌面排版。自己读取精确源码并编辑，使用 page_browser 填入全新的客户信息、点击提交，等待后截图并 reload，再截图检查原任务结果和完整任务编号是否仍显示，不能重复提交。用当前哈希完成静态与模拟预览检查，自己修复常规错误后结束，不让我逐次要求修改。')
  const entries=await allEntries<Entry>(page,token,binding.id)
  expect(observations(entries,created).some(report=>report.image)).toBe(true)
  expect(observations(entries,edited).some(report=>report.image)).toBe(true)
  const draft=await read<{html:string;htmlHash:string}>(page,token,`/applications/${application}/ui/drafts/${binding.uiDraftId}`)
  expect(draft.html).toContain('客户服务台')
  const last=observations(entries,edited).at(-1)!
  expect(last.documentHash).toBe(draft.htmlHash)
  expect(last.report.errors).toEqual([])
  expect(last.report.actions.some(action=>action.status==='failed')).toBe(false)
  const replay=observations(entries,edited).filter(report=>report.documentHash===draft.htmlHash&&report.report.actions.some(action=>action.action==='reload'&&action.status==='completed')).at(-1)
  expect(replay,'The final page must preserve its task across an actual browser reload').toBeTruthy()
  expect(replay!.report.facts?.latestTask?.state).toBe('completed')
  expect(replay!.report.facts?.counts).toMatchObject({invoke:1,formalRequests:0})
  expect(replay!.report.dom.text).toContain(replay!.report.facts!.latestTask!.taskId)
  await page.getByRole('tab',{name:'检查结果',exact:true}).click()
  const checked=(mode:string)=>page.waitForResponse(response=>response.url().endsWith(`/ui/drafts/${binding.uiDraftId}/checks`)&&response.request().method()==='POST'&&response.request().postDataJSON().mode===mode)
  const [staticCheck,previewCheck]=await Promise.all([checked('static'),checked('preview'),page.getByRole('button',{name:'重新检查',exact:true}).click()])
  for(const check of [staticCheck,previewCheck]){expect(check.ok(),await check.text()).toBe(true);expect(check.request().postDataJSON().errors).toEqual([])}
  await expect(page.getByTestId('design-browser-observation').first()).toBeVisible()
  await info.attach('page-design-events.json',{body:JSON.stringify(entries),contentType:'application/json'})
  await info.attach('page.html',{body:draft.html,contentType:'text/html'})
  await page.screenshot({path:info.outputPath('page-authored.png')})
})
