import {expect,test,type Route} from '@playwright/test'
import {readFileSync,writeFileSync} from 'node:fs'
import {join} from 'node:path'

const configuration=process.env.AGENTX_LOCAL_PAGE_EDITOR_CONFIG
test.skip(!configuration,'Requires the explicitly retained local page editor')
test.use({trace:'off',video:'off'})

type Working={id:string;html:string;htmlHash:string;revision:number}
type Session={id:string;runs:Array<{id:string;status:string}>}
type Publication={id:string;revision:number;uiVersionId:string;slug:string;status:string;manifest:{htmlHash:string}}
type Publications={activePublicationId:string;publications:Publication[]}

// Playwright cannot pipe an open HTTPS SSE response through route.fulfill.
// Keep the server's complete SSE frames unchanged and end that connection;
// the actual editor reconnects with its cursor. No events or tools are mocked.
async function relay(route:Route){
  if(!/\/design-agent\/sessions\/[^/]+\/events\?/.test(route.request().url())){
    const response=await route.fetch();await route.fulfill({response});return
  }
  const controller=new AbortController(),timer=setTimeout(()=>controller.abort(),1500)
  let body=''
  try{
    const response=await fetch(route.request().url(),{headers:route.request().headers(),signal:controller.signal})
    if(!response.ok){await route.fulfill({status:response.status,body:await response.text()});return}
    const reader=response.body!.getReader(),decoder=new TextDecoder()
    while(true){const chunk=await reader.read();if(chunk.done)break;body+=decoder.decode(chunk.value,{stream:true})}
  }catch(error){if(!controller.signal.aborted)throw error}finally{clearTimeout(timer);controller.abort()}
  const complete=body.lastIndexOf('\n\n')
  await route.fulfill({status:200,contentType:'text/event-stream',body:complete<0?'':body.slice(0,complete+2)})
}

test('Continuous page editing, publication history preview and live-only rollback',async({page,context},info)=>{
  test.setTimeout(1_500_000)
  const config=JSON.parse(readFileSync(configuration!,'utf8')) as {baseUrl:string;applicationId:string;credentialsFile:string;evidenceDir:string}
  const credentials=JSON.parse(readFileSync(config.credentialsFile,'utf8')) as {username:string;password:string}
  await context.route(/^https:\/\/(?:run\.)?agentx\.localhost\//,relay)
  await page.goto(config.baseUrl+'/login')
  await page.getByLabel('用户名',{exact:true}).fill(credentials.username)
  await page.getByLabel('密码',{exact:true}).fill(credentials.password)
  const logged=page.waitForResponse(response=>response.url().endsWith('/api/v1/auth/login')&&response.request().method()==='POST')
  await page.getByRole('button',{name:'登录',exact:true}).click()
  const login=await logged;expect(login.ok()).toBe(true)
  const token=(await login.json()).accessToken as string
  const get=async<T>(path:string):Promise<T>=>{const response=await page.request.get(config.baseUrl+'/api/v1'+path,{headers:{Authorization:'Bearer '+token}});expect(response.ok(),await response.text()).toBe(true);return response.json()}
  const base='/applications/'+config.applicationId
  const listing=()=>get<Publications>(base+'/ui/publications')
  const working=async()=>{const values=await get<Working[]>(base+'/ui/drafts');expect(values).toHaveLength(1);return values[0]!}
  const initial=await listing(),original=initial.publications.find(value=>value.id===initial.activePublicationId)!
  expect(original).toBeTruthy()
  const originalView=await get<{html:string}>(base+'/ui/publications/'+original.id+'/preview')
  const originalTitle=originalView.html.match(/<h1\b[^>]*>([\s\S]*?)<\/h1>/i)?.[1]?.replace(/<[^>]*>/g,'').trim()
  expect(originalTitle).toBeTruthy()
  const binding=page.waitForResponse(response=>response.url().endsWith('/builder-sessions')&&response.request().method()==='POST')
  await page.goto(config.baseUrl+base+'?tab=ui')
  const bound=await binding;expect(bound.ok(),await bound.text()).toBe(true)
  const session=(await bound.json()).id as string
  const report:Record<string,unknown>={passed:false,sessionId:session,originalPublicationId:original.id,runs:[]}
  const save=()=>writeFileSync(join(config.evidenceDir,'editor-business-test.json'),JSON.stringify(report,null,2))
  save()
  try{
    await expect(page.getByRole('button',{name:'生成新界面',exact:true})).toHaveCount(0)
    await expect(page.getByLabel('界面草稿',{exact:true})).toHaveCount(0)
    await page.getByRole('tab',{name:'HTML 源码',exact:true}).click()
    await expect(page.getByTestId('application-page-source')).toHaveAttribute('readonly')
    await page.getByRole('tab',{name:'模拟预览',exact:true}).click()
    const setting=await get<{status:string;modelAlias:string}>('/platform-ai/model-setting')
    expect(setting.status).toBe('ready');expect(setting.modelAlias).toBe('glm-5.3-flash')
    async function edit(prompt:string){
      const panel=page.getByTestId('application-page-agent')
      await panel.getByRole('textbox',{name:'描述界面调整'}).fill(prompt)
      const started=page.waitForResponse(response=>response.url().endsWith('/application-ui/sessions/'+session+'/runs')&&response.request().method()==='POST')
      await panel.getByRole('button',{name:'发送',exact:true}).click()
      const response=await started;expect(response.ok(),await response.text()).toBe(true)
      const run=(await response.json()) as {id:string;status:string}
      ;(report.runs as unknown[]).push({id:run.id,prompt});save()
      await expect.poll(async()=>{const value=await get<Session>('/application-ui/sessions/'+session);return value.runs.find(item=>item.id===run.id)?.status},{timeout:600_000,intervals:[1000,2000]}).toMatch(/completed|failed|cancelled|outcome_unknown/)
      const final=(await get<Session>('/application-ui/sessions/'+session)).runs.find(item=>item.id===run.id)!
      expect(final.status,JSON.stringify(final)).toBe('completed')
      const html=await working();writeFileSync(join(config.evidenceDir,'editor-'+run.id+'.html'),html.html)
      return html
    }
    async function publish(){
      const accepted=page.waitForResponse(response=>response.url().endsWith(base+'/ui/publications')&&response.request().method()==='POST')
      await page.getByTestId('application-publish-page').click()
      const response=await accepted;expect(response.ok(),await response.text()).toBe(true)
      expect(response.request().postDataJSON().slug).toBe(original.slug)
      await expect(page.getByRole('dialog',{name:'设置访问地址'})).toHaveCount(0)
      const result=(await response.json()) as Publication
      await expect.poll(async()=>(await listing()).activePublicationId,{timeout:120_000}).toBe(result.id)
      return result
    }
    const first=await edit('只将页面正文的主标题改为“费用报销在线服务台”，保留现有全部字段、补充资料、审批结果和历史交互。不要修改工作流或发布。完成静态检查和模拟检查，不需要提问。')
    expect(first.html).toContain('费用报销在线服务台')
    const published=await publish();report.published=published;save()
    const subtitle=first.html.includes('填写报销信息，补齐资料，在线跟踪审批结果。')?'提交报销申请，补充资料并查看审批结果。':'填写报销信息，补齐资料，在线跟踪审批结果。'
    const current=await edit(`只将主标题下的说明改为“${subtitle}”，保留现有主标题和全部业务功能。请实际修改 HTML 并完成静态检查和模拟检查，不发布，不需要提问。`)
    expect(current.htmlHash).not.toBe(first.htmlHash)
    const before=await get<Session>('/application-ui/sessions/'+session)
    await page.screenshot({path:info.outputPath('continuous-page-editor.jpg'),type:'jpeg',fullPage:true})
    await page.getByTestId('application-page-history').click()
    await page.getByTestId('page-history-'+original.id).click()
    const history=page.getByTestId('application-page-history-preview')
    await expect(history).toBeVisible()
    await expect(history.frameLocator('iframe').frameLocator('iframe[name="agentx-application-page"]').getByRole('heading',{name:originalTitle,exact:true})).toBeVisible()
    expect(await working()).toEqual(current)
    await page.screenshot({path:info.outputPath('page-publication-history.jpg'),type:'jpeg',fullPage:true})
    await page.getByRole('button',{name:'回滚到此记录',exact:true}).click()
    const rolled=page.waitForResponse(response=>response.url().endsWith(base+'/ui/publications')&&response.request().method()==='POST')
    await page.getByRole('button',{name:'确认回滚',exact:true}).click()
    const rollback=await rolled;expect(rollback.ok(),await rollback.text()).toBe(true)
    expect(rollback.request().postDataJSON()).toMatchObject({draftId:null,uiVersionId:original.uiVersionId,slug:original.slug})
    const rollbackId=(await rollback.json()).id as string
    await expect.poll(async()=>(await listing()).activePublicationId,{timeout:120_000}).toBe(rollbackId)
    expect(await working()).toEqual(current)
    expect((await get<Session>('/application-ui/sessions/'+session)).runs).toEqual(before.runs)
    report.rollback={id:rollbackId,workingHtmlUnchanged:true,conversationRunsUnchanged:true};save()
    await page.getByRole('button',{name:'关闭',exact:true}).click()
    await page.close()
    page=await context.newPage()
    const rebound=page.waitForResponse(response=>response.url().endsWith('/builder-sessions')&&response.request().method()==='POST')
    await page.goto(config.baseUrl+base+'?tab=ui')
    const reopened=await rebound;expect(reopened.ok(),await reopened.text()).toBe(true)
    expect((await reopened.json()).id).toBe(session)
    await expect(page.getByTestId('application-page-agent')).toContainText('费用报销在线服务台')
    const retained=await working();expect(retained.id).toBe(current.id);expect(retained.html).toBe(current.html)
    report.reopened={sameSession:true,sameHtml:true,newWindowFencesPreviousBinding:true}
    const republished=await publish();report.republished=republished;expect((await working()).html).toBe(current.html)
    await page.getByRole('tab',{name:'模拟预览',exact:true}).click()
    await expect(page.getByTestId('application-page-agent').getByRole('button',{name:'发送',exact:true})).toBeDisabled()
    await page.screenshot({path:info.outputPath('continuous-page-editor-final.jpg'),type:'jpeg',fullPage:true})
    report.passed=true;save()
  }finally{save()}
})

test('Visible editor controls preserve HTML and conversation through publishing and rollback without model calls',async({page,context},info)=>{
  const config=JSON.parse(readFileSync(configuration!,'utf8')) as {baseUrl:string;applicationId:string;credentialsFile:string;evidenceDir:string}
  const credentials=JSON.parse(readFileSync(config.credentialsFile,'utf8'))
  await context.route(/^https:\/\/(?:run\.)?agentx\.localhost\//,relay)
  await page.goto(config.baseUrl+'/login')
  await page.getByLabel('用户名',{exact:true}).fill(credentials.username)
  await page.getByLabel('密码',{exact:true}).fill(credentials.password)
  const logged=page.waitForResponse(response=>response.url().endsWith('/api/v1/auth/login')&&response.request().method()==='POST')
  await page.getByRole('button',{name:'登录',exact:true}).click()
  const token=(await(await logged).json()).accessToken as string
  const base='/applications/'+config.applicationId
  const get=async<T>(path:string):Promise<T>=>{const response=await page.request.get(config.baseUrl+'/api/v1'+path,{headers:{Authorization:'Bearer '+token}});expect(response.ok(),await response.text()).toBe(true);return response.json()}
  const bound=page.waitForResponse(response=>response.url().endsWith('/builder-sessions')&&response.request().method()==='POST')
  await page.goto(config.baseUrl+base+'?tab=ui')
  const binding=await bound;expect(binding.ok(),await binding.text()).toBe(true)
  const session=(await binding.json()).id as string
  const before=(await get<Working[]>(base+'/ui/drafts'))[0]!
  const beforeRuns=(await get<Session>('/application-ui/sessions/'+session)).runs
  const current=await get<Publications>(base+'/ui/publications')
  const target=current.publications.findLast(record=>record.status==='superseded')!
  expect(target).toBeTruthy()
  const unexpected:string[]=[]
  page.on('request',request=>{if(request.method()==='POST'&&(/\/application-ui\/sessions\/[^/]+\/runs$/.test(request.url())||/\/gateway\/v1\/pages\/[^/]+\/invocations$/.test(request.url())))unexpected.push(new URL(request.url()).pathname)})
  const agent=page.getByTestId('application-page-agent')
  for(const size of [{width:1440,height:900},{width:1280,height:720}]){
    await page.setViewportSize(size)
    const input=agent.getByRole('textbox',{name:'描述界面调整'})
    await expect(input).toBeVisible()
    const bounds=await input.boundingBox();expect(bounds!.y+bounds!.height).toBeLessThanOrEqual(size.height)
    const send=await agent.getByRole('button',{name:'发送',exact:true}).boundingBox();expect(send!.y+send!.height).toBeLessThanOrEqual(size.height)
  }
  await page.setViewportSize({width:1440,height:900})
  await page.getByTestId('application-page-history').click()
  const preview=page.waitForResponse(response=>response.url().endsWith('/ui/publications/'+target.id+'/preview'))
  await page.getByTestId('page-history-'+target.id).click()
  await expect(page.getByTestId('application-page-history-preview')).toBeVisible()
  const frozen=await preview;expect(frozen.ok()).toBe(true)
  expect((await frozen.json()).publication.publicationId).toBe(target.id)
  await page.screenshot({path:info.outputPath('page-publication-history.jpg'),type:'jpeg',fullPage:true})
  await page.getByRole('button',{name:'回滚到此记录',exact:true}).click()
  const rollback=page.waitForResponse(response=>response.url().endsWith(base+'/ui/publications')&&response.request().method()==='POST')
  await page.getByRole('button',{name:'确认回滚',exact:true}).click()
  const changed=await rollback;expect(changed.ok()).toBe(true)
  const rollbackId=(await changed.json()).id
  await expect.poll(async()=>(await get<Publications>(base+'/ui/publications')).activePublicationId,{timeout:120_000}).toBe(rollbackId)
  expect((await get<Working[]>(base+'/ui/drafts'))[0]).toEqual(before)
  expect((await get<Session>('/application-ui/sessions/'+session)).runs).toEqual(beforeRuns)
  await page.getByRole('button',{name:'关闭',exact:true}).click()
  const publishing=page.waitForResponse(response=>response.url().endsWith(base+'/ui/publications')&&response.request().method()==='POST')
  await page.getByTestId('application-publish-page').click()
  const accepted=await publishing;expect(accepted.ok(),await accepted.text()).toBe(true)
  const publication=(await accepted.json()) as Publication
  await expect.poll(async()=>(await get<Publications>(base+'/ui/publications')).activePublicationId,{timeout:120_000}).toBe(publication.id)
  expect(publication.slug).toBe('expense-demo')
  expect((await get<Working[]>(base+'/ui/drafts'))[0]).toEqual(before)
  expect((await get<Session>('/application-ui/sessions/'+session)).runs).toEqual(beforeRuns)
  expect(unexpected).toEqual([])
  await page.getByRole('tab',{name:'模拟预览',exact:true}).click()
  await page.screenshot({path:info.outputPath('continuous-page-editor-final.jpg'),type:'jpeg',fullPage:true})
  writeFileSync(join(config.evidenceDir,'editor-layout-verification.json'),JSON.stringify({passed:true,sessionId:session,workingHtmlHash:before.htmlHash,publicationId:publication.id,viewportSizes:['1440x900','1280x720'],currentHtmlAndRunsUnchanged:true,unexpectedModelOrBusinessRequests:unexpected},null,2))
})
