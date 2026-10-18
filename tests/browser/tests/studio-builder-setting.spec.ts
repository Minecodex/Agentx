import { expect, type Page } from '@playwright/test'
import { test, completed, count, create, createVersion, editorPath, login, read, saved, send, setModel, type Draft } from '../studio-builder-support'

type Setting={version:number;modelAliasId:string|null;status:string;canManage:boolean;reason:string|null}
const setting=(page:Page,token:string)=>read<Setting>(page,token,'/platform-ai/model-setting')

async function modelStatus(page:Page,token:string,id:string,status:string,overrides:Record<string,unknown>={}) {
  const model=await read<Record<string,unknown>>(page,token,`/models/aliases/${id}`)
  const price=await read<Array<{currency:string;inputPerMillion:string;outputPerMillion:string}>>(page,token,`/models/deployments/${model.deploymentId}/prices`)
  const keys=['connectionName','providerType','endpoint','credentialId','ownerDepartmentId','alias','modelName','maxInputTokens','maxOutputTokens','defaultParameters','capabilities']
  const response=await page.request.patch(`/api/v1/models/aliases/${id}`,{headers:{Authorization:`Bearer ${token}`},data:{...Object.fromEntries(keys.map(key=>[key,model[key]])),...overrides,expectedAliasVersion:model.aliasVersion,status,price:{currency:price[0]!.currency,inputPerMillion:price[0]!.inputPerMillion,outputPerMillion:price[0]!.outputPerMillion}}})
  expect(response.ok(),await response.text()).toBeTruthy()
}

async function loginAs(page:Page,kind:'EDITOR'|'VIEWER') {
  await page.goto('/login')
  await page.getByLabel('用户名').fill(process.env[`AGENTX_E2E_STUDIO_${kind}_USER`]!)
  await page.getByLabel('密码').fill(process.env[`AGENTX_E2E_STUDIO_${kind}_PASSWORD`]!)
  const pending=page.waitForResponse(response=>response.url().endsWith('/auth/login')&&response.request().method()==='POST')
  await page.getByRole('button',{name:'登录',exact:true}).click()
  await expect(page).toHaveURL(/\/$/)
  return (await (await pending).json()).accessToken as string
}

test('P8-MODEL-01/P8-E2E-16: tenant model is shared with workflow editors; model administration and readonly access remain separate',async({page,browser})=>{
  const token=await login(page)
  await setModel(page,process.env.AGENTX_E2E_STUDIO_FIXTURE_ALIAS!)
  const value=await setting(page,token)
  const created=await create(page,`P8 tenant sharing ${Date.now()}`)
  const child=await create(page,`P8 private child ${Date.now()}`)
  const metadata=await read<{name:string;version:number}>(page,token,`/workflows/${child.id}`)
  const privateChild=await page.request.patch(`/api/v1/workflows/${child.id}`,{headers:{Authorization:`Bearer ${token}`},data:{name:metadata.name,description:null,visibility:'private',version:metadata.version}})
  expect(privateChild.ok(),await privateChild.text()).toBeTruthy()
  const childVersion=await createVersion(page,child)
  for(const [kind,role] of [['EDITOR','editor'],['VIEWER','viewer']] as const){
    const response=await page.request.post(`/api/v1/workflows/${created.id}/members`,{headers:{Authorization:`Bearer ${token}`},data:{userId:process.env[`AGENTX_E2E_STUDIO_${kind}_ID`],memberRole:role}})
    expect(response.ok(),await response.text()).toBeTruthy()
  }
  const otherContext=await browser.newContext({baseURL:test.info().project.use.baseURL,viewport:{width:1440,height:900}})
  const other=await otherContext.newPage()
  let editorSession=''
  let editorAck:Record<string,unknown>|undefined
  other.on('request',request=>{
    const match=request.url().match(/\/studio-builder\/sessions\/([^/]+)\/runs$/)
    if(match)editorSession=match[1]!
    if(request.url().endsWith('/acknowledgements')&&request.method()==='POST')editorAck=request.postDataJSON() as Record<string,unknown>
  })
  const editor=await loginAs(other,'EDITOR')
  expect(await setting(other,editor)).toMatchObject({modelAliasId:value.modelAliasId,canManage:false,status:'ready'})
  const denied=await other.request.put('/api/v1/platform-ai/model-setting',{headers:{Authorization:`Bearer ${editor}`},data:{expectedVersion:value.version,modelAliasId:null}})
  expect(denied.status()).toBe(403)
  await other.goto(editorPath(created))
  await other.getByRole('button',{name:'AI 创作',exact:true}).click()
  await send(other,'P8FIX: label=SharedByEditor')
  await completed(other)
  await saved(other,editor,created)
  expect(editorAck).toBeTruthy()
  for(const suffix of ['', '/entries', `/commands/${editorAck!.commandId}`]){
    const inaccessible=await page.request.get(`/api/v1/studio-builder/sessions/${editorSession}${suffix}`,{headers:{Authorization:`Bearer ${token}`}})
    expect(inaccessible.status(),await inaccessible.text()).toBe(404)
  }
  const foreignAck=await page.request.post(`/api/v1/studio-builder/sessions/${editorSession}/acknowledgements`,{headers:{Authorization:`Bearer ${token}`},data:editorAck})
  expect(foreignAck.status(),await foreignAck.text()).toBe(404)
  const parent=await read<{definition:{nodes:Record<string,unknown>[]}}>(other,editor,`/workflows/${created.id}/drafts/${created.documentId}`)
  parent.definition.nodes.push({id:'private-child',key:'private_child',type:'sub_workflow',typeVersion:1,name:'Private child',parameters:{workflowVersionId:childVersion.id,inputs:{kind:'object',fields:{}}},resourceReferences:[],settings:{}})
  const hidden=await other.request.post(`/api/v1/workflows/${created.id}/drafts/${created.documentId}/resolve-plugins`,{headers:{Authorization:`Bearer ${editor}`},data:{definition:parent.definition}})
  expect(hidden.status(),await hidden.text()).toBe(404)
  expect(await count(other,editor,created.id)).toBe(0)
  await otherContext.close()
  const viewerContext=await browser.newContext({baseURL:test.info().project.use.baseURL,viewport:{width:1440,height:900}})
  const viewerPage=await viewerContext.newPage()
  const viewer=await loginAs(viewerPage,'VIEWER')
  await viewerPage.goto(`/workflows/${created.id}/versions/${created.createdVersionId}`)
  await expect(viewerPage.getByTestId('workflow-canvas')).toBeVisible()
  await expect(viewerPage.getByRole('button',{name:'AI 创作',exact:true})).toHaveCount(0)
  const derive=await viewerPage.request.post(`/api/v1/workflows/${created.id}/versions`,{headers:{Authorization:`Bearer ${viewer}`}})
  expect(derive.status()).toBe(403)
  await viewerContext.close()
})

test('P8-E2E-17: disabled credentials cancel pending work; credential replacement requires a new explicit tenant authorization',async({page})=>{
  const token=await login(page)
  const id=process.env.AGENTX_E2E_STUDIO_FIXTURE_ID!
  const alias=process.env.AGENTX_E2E_STUDIO_FIXTURE_ALIAS!
  const model=await read<{credentialId:string}>(page,token,`/models/aliases/${id}`)
  const alternate=await read<{credentialId:string}>(page,token,`/models/aliases/${process.env.AGENTX_E2E_STUDIO_ALTERNATE_ID}`)
  await setModel(page,alias)
  const created=await create(page,`P8 revoke ${Date.now()}`)
  let session=''
  page.on('request',request=>{const match=request.url().match(/\/studio-builder\/sessions\/([^/]+)\/runs$/);if(match)session=match[1]!})
  await page.getByRole('button',{name:'AI 创作',exact:true}).click()
  const run=await send(page,'P8FIX: question label=Revoked')
  await expect(page.getByTestId('studio-builder-panel').getByText('Which label should the node use?',{exact:true})).toBeVisible()
  const credential=await read<{name:string;version:number}>(page,token,`/credentials/${model.credentialId}`)
  const changed=await page.request.patch(`/api/v1/credentials/${model.credentialId}`,{headers:{Authorization:`Bearer ${token}`},data:{name:credential.name,version:credential.version,status:'disabled'}})
  expect(changed.ok(),await changed.text()).toBeTruthy()
  await expect.poll(async()=> (await read<{runs:Array<{id:string;status:string}>}>(page,token,`/studio-builder/sessions/${session}`)).runs.find(item=>item.id===run)?.status).toBe('cancelled')
  expect(await setting(page,token)).toMatchObject({status:'unavailable',reason:'credential_disabled'})
  const reenabled=await page.request.patch(`/api/v1/credentials/${model.credentialId}`,{headers:{Authorization:`Bearer ${token}`},data:{name:credential.name,version:(await changed.json()).version,status:'active'}})
  expect(reenabled.ok(),await reenabled.text()).toBeTruthy()
  await modelStatus(page,token,id,'active',{credentialId:alternate.credentialId})
  expect(await setting(page,token)).toMatchObject({status:'unavailable',reason:'credential_changed'})
  await setModel(page,alias)
  expect(await setting(page,token)).toMatchObject({status:'ready'})
  await modelStatus(page,token,id,'active',{credentialId:model.credentialId})
  await setModel(page,alias)
  await page.goto(editorPath(created))
  await page.getByRole('button',{name:'AI 创作',exact:true}).click()
  await send(page,'P8FIX: label=Reauthorized')
  await completed(page)
  const before=await saved(page,token,created)
  const current=await read<{revision:number;definition:{nodes:Array<Record<string,unknown>>};editorDocument:unknown}>(page,token,`/workflows/${created.id}/drafts/${created.documentId}`)
  current.definition.nodes.find(node=>node.id==='p8-decision')!.resourceReferences=[{resourceType:'model',resourceId:'invalid-reference',operation:'use'}]
  const rejected=await page.request.put(`/api/v1/workflows/${created.id}/drafts/${created.documentId}`,{headers:{Authorization:`Bearer ${token}`},data:{expectedRevision:current.revision,definition:current.definition,editorDocument:current.editorDocument}})
  expect(rejected.status(),await rejected.text()).toBe(422)
  expect((await read<Draft>(page,token,`/workflows/${created.id}/drafts/${created.documentId}`)).revision).toBe(before.revision)
  expect(await count(page,token,created.id)).toBe(0)
})

test('P8-MODEL-02: explicit clearing, disabled model, deletion protection and manual editing remain usable',async({page})=>{
  const token=await login(page)
  const alias=process.env.AGENTX_E2E_STUDIO_FIXTURE_ALIAS!
  const id=process.env.AGENTX_E2E_STUDIO_FIXTURE_ID!
  await setModel(page,alias)
  const blocked=await page.request.delete(`/api/v1/models/aliases/${id}`,{headers:{Authorization:`Bearer ${token}`}})
  expect(blocked.status()).toBe(409)
  const row=page.getByRole('row').filter({hasText:alias})
  const cleared=page.waitForResponse(response=>response.url().endsWith('/platform-ai/model-setting')&&response.request().method()==='PUT')
  await row.getByRole('button',{name:'清除设置',exact:true}).click()
  expect((await cleared).status()).toBe(200)
  expect(await setting(page,token)).toMatchObject({modelAliasId:null,status:'unconfigured'})
  const created=await create(page,`P8 no model ${Date.now()}`)
  await page.getByRole('button',{name:'AI 创作',exact:true}).click()
  const panel=page.getByTestId('studio-builder-panel')
  await panel.getByLabel('描述工作流需求').fill('Create')
  await expect(panel.getByRole('button',{name:'发送',exact:true})).toBeDisabled()
  await panel.getByRole('button',{name:'关闭',exact:true}).click()
  await page.getByTestId('node-creator').getByRole('textbox',{name:'搜索节点'}).fill('if')
  await page.getByTestId('palette-action-if').click()
  await saved(page,token,created)
  await setModel(page,alias)
  await modelStatus(page,token,id,'disabled')
  expect(await setting(page,token)).toMatchObject({status:'unavailable',reason:'model_disabled'})
  await modelStatus(page,token,id,'active')
  expect(await count(page,token,created.id)).toBe(0)
})

test('P8-MODEL-03: CAS conflicts and model snapshots across old and new runs',async({page,context})=>{
  const token=await login(page)
  await setModel(page,process.env.AGENTX_E2E_STUDIO_FIXTURE_ALIAS!)
  const first=await setting(page,token)
  const headers={Authorization:`Bearer ${token}`}
  const competing=await Promise.all([page.request.put('/api/v1/platform-ai/model-setting',{headers,data:{expectedVersion:first.version,modelAliasId:process.env.AGENTX_E2E_STUDIO_FIXTURE_ID}}),page.request.put('/api/v1/platform-ai/model-setting',{headers,data:{expectedVersion:first.version,modelAliasId:process.env.AGENTX_E2E_STUDIO_ALTERNATE_ID}})])
  expect(competing.map(value=>value.status()).sort()).toEqual([200,409])
  await setModel(page,process.env.AGENTX_E2E_STUDIO_FIXTURE_ALIAS!)
  const created=await create(page,`P8 frozen models ${Date.now()}`)
  let session=''
  page.on('request',request=>{const match=request.url().match(/\/studio-builder\/sessions\/([^/]+)\/runs$/);if(match)session=match[1]!})
  await page.getByRole('button',{name:'AI 创作',exact:true}).click()
  const old=await send(page,'P8FIX: question label=OriginalModel')
  await expect(page.getByTestId('studio-builder-panel').getByText('Which label should the node use?',{exact:true})).toBeVisible()
  const modelPage=await context.newPage()
  await setModel(modelPage,process.env.AGENTX_E2E_STUDIO_ALTERNATE_ALIAS!)
  await page.getByTestId('studio-builder-panel').getByRole('button',{name:'Approved',exact:true}).click()
  await completed(page)
  const next=await send(page,'P8FIX: label=NextModel')
  await completed(page)
  const summary=await read<{runs:Array<{id:string;modelAliasId:string;settingVersion:number}>}>(page,token,`/studio-builder/sessions/${session}`)
  expect(summary.runs.find(run=>run.id===old)?.modelAliasId).toBe(process.env.AGENTX_E2E_STUDIO_FIXTURE_ID)
  expect(summary.runs.find(run=>run.id===next)?.modelAliasId).toBe(process.env.AGENTX_E2E_STUDIO_ALTERNATE_ID)
  expect(summary.runs.find(run=>run.id===next)!.settingVersion).toBeGreaterThan(summary.runs.find(run=>run.id===old)!.settingVersion)
  expect(await count(page,token,created.id)).toBe(0)
  await modelPage.close()
})
