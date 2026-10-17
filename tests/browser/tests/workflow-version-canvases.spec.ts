import { expect, type Page } from '@playwright/test'
import { test, login, create, newCanvas, editorPath, read, saved, publishVersion, manualRun } from '../studio-builder-support'

type Version={id:string;versionNumber:number;publicationStatus:string;hasDraft:boolean;baseVersionId:string|null;definition:unknown;editorDocument:unknown;draftRevision:number}
async function moveExit(page:Page,dx:number) {
  const exit=page.getByTestId('exit-node-exit')
  const box=await exit.boundingBox()
  expect(box).toBeTruthy()
  await page.mouse.move(box!.x+box!.width/2,box!.y+box!.height/2)
  await page.mouse.down()
  await page.mouse.move(box!.x+box!.width/2+dx,box!.y+box!.height/2+35,{steps:12})
  await page.mouse.up()
}
async function restore(page:Page,confirm:boolean) {
  await page.getByRole('button',{name:'还原',exact:true}).click()
  const dialog=page.getByRole('dialog',{name:'还原画布'})
  await expect(dialog).toBeVisible()
  if (!confirm) {await dialog.getByRole('button',{name:'取消',exact:true}).click();return}
  const pending=page.waitForResponse(response=>response.url().endsWith('/restore')&&response.request().method()==='POST')
  await dialog.getByRole('button',{name:'确认还原',exact:true}).click()
  const response=await pending
  expect(response.status(),await response.text()).toBe(200)
  await expect(dialog).not.toBeVisible()
}

test('Version canvases: unpublished save, frozen details, draft publication, restore, archive, history and new canvas',async({page,context},info)=>{
  const token=await login(page)
  const created=await create(page,`Version canvases ${Date.now()}`)
  const root=`/workflows/${created.id}`
  let versions=await read<Version[]>(page,token,`${root}/versions`)
  expect(versions).toHaveLength(1)
  expect(versions[0]).toMatchObject({versionNumber:1,publicationStatus:'unpublished',hasDraft:false})
  await moveExit(page,90)
  const initial=await saved(page,token,created)
  const v1Run=await manualRun(page,token,created)
  const before=await read<Version>(page,token,`${root}/versions/${created.createdVersionId}`)
  expect(before.definition).toEqual(initial.definition)
  await publishVersion(page,token,created,created.createdVersionId)
  expect(await read<Version[]>(page,token,`${root}/versions`)).toHaveLength(1)
  await page.goto(editorPath(created))
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  await saved(page,token,created)
  expect((await read<Version>(page,token,`${root}/versions/${created.createdVersionId}`)).hasDraft).toBe(false)
  await moveExit(page,-90)
  const changed=await saved(page,token,created)
  const frozen=await read<Version>(page,token,`${root}/versions/${created.createdVersionId}`)
  expect(frozen.editorDocument).toEqual(before.editorDocument)
  expect(frozen.hasDraft).toBe(true)
  await page.goto(`${root}/versions/${created.createdVersionId}`)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  for(const name of ['保存','运行','还原','AI 创作'])await expect(page.getByRole('button',{name,exact:true})).toHaveCount(0)
  await expect(page.getByTestId('node-creator-shell')).toHaveCount(0)
  await page.getByTestId('exit-node-exit').click()
  await expect(page.getByTestId('exit-panel').getByTestId('completion-first_return')).toBeDisabled()
  await expect(page.getByTestId('exit-panel').locator('fieldset')).toHaveAttribute('disabled','')
  await page.keyboard.press('Delete')
  await page.keyboard.press('Meta+s')
  await expect(page.getByTestId('exit-node-exit')).toBeVisible()
  expect((await read<Version>(page,token,`${root}/versions/${created.createdVersionId}`)).editorDocument).toEqual(before.editorDocument)
  const deployment=await publishVersion(page,token,created,created.createdVersionId)
  const v2=await read<Version>(page,token,`${root}/versions/${deployment.workflowVersionId}`)
  expect(v2).toMatchObject({versionNumber:2,baseVersionId:created.createdVersionId,publicationStatus:'published'})
  expect(v2.definition).toEqual(changed.definition)
  expect((await read<Version>(page,token,`${root}/versions/${created.createdVersionId}`)).hasDraft).toBe(false)
  await page.goto(root)
  await page.getByTestId('workflow-version-2').getByRole('link',{name:'编辑',exact:true}).click()
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  const contextResponse=await page.request.post(`/api/v1${root}/versions/${v2.id}/edit`,{headers:{Authorization:`Bearer ${token}`}})
  const document=(await contextResponse.json()).document as {id:string;revision:number;editorDocument:unknown}
  const editor={...created,createdVersionId:v2.id,documentId:document.id}
  const v2Run=await manualRun(page,token,editor)
  const other=await context.newPage()
  await other.goto(editorPath(editor))
  await expect(other.getByTestId('workflow-canvas')).toBeVisible()
  await moveExit(page,80)
  await restore(page,false)
  await restore(page,true)
  await expect(page.getByRole('button',{name:'撤销',exact:true})).toBeDisabled()
  const restored=await read<{revision:number;editorDocument:unknown}>(page,token,`${root}/drafts/${document.id}`)
  expect(restored.editorDocument).toEqual(v2.editorDocument)
  expect(restored.revision).toBeGreaterThan(document.revision)
  await moveExit(other,60)
  await expect(other.getByRole('dialog',{name:'草稿版本冲突'})).toBeVisible()
  await other.close()
  await page.reload()
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  expect((await read<Version>(page,token,`${root}/versions/${v2.id}`)).hasDraft).toBe(false)
  await page.goto(root)
  const row=page.getByTestId('workflow-version-2')
  await row.getByRole('button',{name:'v2 更多操作'}).click()
  await page.getByRole('menuitem',{name:'归档',exact:true}).click()
  await expect(row.getByRole('button',{name:'编辑',exact:true})).toBeDisabled()
  await row.getByRole('button',{name:'v2 更多操作'}).click()
  await page.getByRole('menuitem',{name:'取消归档',exact:true}).click()
  await expect(row.getByRole('link',{name:'编辑',exact:true})).toBeVisible()
  await row.getByRole('link',{name:'v1',exact:true}).click()
  await expect(page).toHaveURL(new RegExp(`/versions/${created.createdVersionId}$`))
  await page.goto(root)
  await page.getByRole('button',{name:'查看发布记录',exact:true}).click()
  const history=page.getByRole('dialog',{name:'环境发布记录'})
  const rollback=history.getByRole('button',{name:'回滚到 v1',exact:true}).first()
  await rollback.click()
  await expect(history).not.toBeVisible()
  expect((await read<Array<{status:string;workflowVersionId:string}>>(page,token,`${root}/deployments`)).find(item=>item.status==='active')?.workflowVersionId).toBe(created.createdVersionId)
  const blank=await newCanvas(page,created)
  expect(blank.createdVersionId).not.toBe(v2.id)
  await expect(page.getByTestId('workflow-canvas')).toBeVisible()
  await moveExit(page,70)
  await restore(page,true)
  versions=await read<Version[]>(page,token,`${root}/versions`)
  expect(versions.map(value=>value.versionNumber)).toEqual([3,2,1])
  expect(versions[0]).toMatchObject({publicationStatus:'unpublished',baseVersionId:null})
  await page.goto(root)
  await expect(page.getByTestId('workflow-version-3')).toBeVisible()
  await page.screenshot({path:info.outputPath('version-list.png'),fullPage:true})
  await page.getByTestId('workflow-version-2').getByRole('button',{name:'v2 更多操作'}).click()
  await page.getByRole('menuitem',{name:'查看运行记录',exact:true}).click()
  await expect(page).toHaveURL(new RegExp(`workflowVersionIds=${v2.id}`))
  const executions=await read<{items:Array<{id:string}>}>(page,token,`/executions?workflowIds=${created.id}&workflowVersionIds=${v2.id}&pageSize=100`)
  expect(executions.items.some(execution=>execution.id===v2Run.id)).toBe(true)
  expect(executions.items.some(execution=>execution.id===v1Run.id)).toBe(false)
})


test('Version canvas restore waits for an in-flight save and uses its acknowledged revision',async({page})=>{
  const token=await login(page)
  const created=await create(page,`Version restore race ${Date.now()}`)
  const root=`/workflows/${created.id}`
  const initial=await read<{revision:number;editorDocument:unknown}>(page,token,`${root}/drafts/${created.documentId}`)
  let release!:()=>void
  let held!:()=>void
  const pending=new Promise<void>(resolve=>{held=resolve})
  const gate=new Promise<void>(resolve=>{release=resolve})
  let intercepted=false
  let restoreRequests=0
  page.on('request',request=>{if(request.url().endsWith('/restore')&&request.method()==='POST')restoreRequests++})
  await page.route(`**${root}/drafts/${created.documentId}`,async route=>{
    if(route.request().method()!=='PUT'||intercepted){await route.continue();return}
    intercepted=true
    const response=await route.fetch()
    held()
    await gate
    await route.fulfill({response})
  })
  try {
    await moveExit(page,80)
    await pending
    await page.getByRole('button',{name:'还原',exact:true}).click()
    const dialog=page.getByRole('dialog',{name:'还原画布'})
    await dialog.getByRole('button',{name:'确认还原',exact:true}).click()
    await expect(dialog.getByRole('button',{name:'正在还原…',exact:true})).toBeDisabled()
    expect(restoreRequests).toBe(0)
    release()
    await expect(dialog).not.toBeVisible()
    expect(restoreRequests).toBe(1)
    const restored=await read<{revision:number;editorDocument:unknown}>(page,token,`${root}/drafts/${created.documentId}`)
    expect(restored.revision).toBe(initial.revision+2)
    expect(restored.editorDocument).toEqual(initial.editorDocument)
    await expect(page.getByRole('button',{name:'撤销',exact:true})).toBeDisabled()
    await page.reload()
    await expect(page.getByTestId('workflow-canvas')).toBeVisible()
    expect((await read<{editorDocument:unknown}>(page,token,`${root}/drafts/${created.documentId}`)).editorDocument).toEqual(initial.editorDocument)
  } finally {release()}
})

test('Version list: member management and desktop locale/theme variants',async({page},info)=>{
  const token=await login(page)
  const created=await create(page,`Version members ${Date.now()}`)
  const me=await read<{departmentId:string}>(page,token,'/auth/me')
  const roles=await read<{items:Array<{id:string;code:string}>}>(page,token,'/roles')
  const username=`version-member-${Date.now()}`
  const response=await page.request.post('/api/v1/users',{headers:{Authorization:`Bearer ${token}`},data:{username,displayName:'Version Member',departmentId:me.departmentId,roleId:roles.items.find(role=>role.code==='company_admin')!.id}})
  expect(response.status(),await response.text()).toBe(201)
  const user=await response.json() as {id:string}
  const root=`/workflows/${created.id}`
  await page.goto(root)
  await page.getByRole('button',{name:'成员管理',exact:true}).click()
  const dialog=page.getByRole('dialog',{name:'成员管理',exact:true})
  await dialog.getByRole('combobox',{name:'添加成员',exact:true}).click()
  await page.getByRole('option',{name:`Version Member · ${username}`,exact:true}).click()
  await dialog.getByRole('button',{name:'添加成员',exact:true}).click()
  const role=dialog.getByRole('combobox',{name:'Version Member的角色',exact:true})
  await expect(role).toHaveText('查看者')
  await role.click()
  await page.getByRole('option',{name:'编辑者',exact:true}).click()
  await expect.poll(async()=>(await read<Array<{userId:string;memberRole:string}>>(page,token,`${root}/members`)).find(member=>member.userId===user.id)?.memberRole).toBe('editor')
  await dialog.getByRole('button',{name:'移除',exact:true}).click()
  await expect(role).toHaveCount(0)
  expect((await read<Array<{userId:string}>>(page,token,`${root}/members`)).some(member=>member.userId===user.id)).toBe(false)
  await dialog.getByRole('button',{name:'关闭',exact:true}).click()
  for(const locale of ['zh-CN','en-US']){
    if(await page.locator('html').getAttribute('lang')!==locale){
      await page.getByRole('button',{name:/^(语言|Language)$/}).click()
      await page.getByRole('menuitemradio',{name:locale==='zh-CN'?'简体中文':'English'}).click()
    }
    for(const theme of ['light','dark']){
      await page.getByRole('button',{name:/^(主题|Theme)$/}).click()
      await page.getByRole('menuitemradio',{name:theme==='light'?/^(浅色|Light)$/:/^(深色|Dark)$/}).click()
      await expect(page.getByTestId('workflow-version-1')).toBeVisible()
      expect(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth)).toBe(true)
      await page.screenshot({path:info.outputPath(`versions-${locale}-${theme}.png`),fullPage:true})
    }
  }
})

test('Version publish controls: header actions, environment choice, missing environments and retry', async ({ page, context }, info) => {
  const token = await login(page)
  const name = `Publish controls ${Date.now()}`
  const created = await create(page, name)
  const root = `/workflows/${created.id}`
  const environments = await read<Array<{ id: string; name: string; code: string; status: string }>>(page, token, '/environments')
  const production = environments.find(environment => environment.code === 'production')!
  expect(production).toBeTruthy()
  await page.goto(root)
  const header = page.locator('header').filter({ has: page.getByRole('heading', { name, exact: true }) })
  const members = header.getByRole('button', { name: '成员管理', exact: true })
  const canvas = header.getByRole('button', { name: '新建画布', exact: true })
  await expect(members).toBeVisible()
  await expect(canvas).toBeVisible()
  expect((await canvas.boundingBox())!.x).toBeGreaterThan((await members.boundingBox())!.x)
  await expect(page.getByRole('button', { name: '新建画布', exact: true })).toHaveCount(1)
  const row = page.getByTestId('workflow-version-1')
  const publish = row.getByRole('button', { name: '发布', exact: true })
  const style = async (locator: ReturnType<Page['getByRole']>) => locator.evaluate(element => {
    const value = getComputedStyle(element)
    return [value.borderWidth, value.backgroundColor, value.color, value.fontSize, value.height]
  })
  expect(await style(publish)).toEqual(await style(row.getByRole('link', { name: '编辑', exact: true })))
  await publish.click()
  const dialog = page.getByRole('dialog', { name: '发布到环境…', exact: true })
  const environmentSelect = dialog.getByRole('combobox', { name: '目标环境', exact: true })
  await environmentSelect.click()
  for (const environment of environments.filter(item => item.status === 'active')) {
    await expect(page.getByRole('option', { name: environment.name, exact: true })).toBeVisible()
  }
  await page.getByRole('option', { name: production.name, exact: true }).click()
  await page.screenshot({ path: info.outputPath('publish-environment-selection.png'), fullPage: true })
  const deploymentResponse = page.waitForResponse(response => response.url().endsWith(`${root}/deployments`) && response.request().method() === 'POST')
  await dialog.getByRole('button', { name: '发布到环境…', exact: true }).click()
  const response = await deploymentResponse
  expect(response.status(), await response.text()).toBe(200)
  expect((await response.json()).environmentId).toBe(production.id)
  await expect(dialog).not.toBeVisible()
  await page.screenshot({ path: info.outputPath('workflow-header-and-actions.png'), fullPage: true })

  const setEnvironmentStatus = async (target: Page, environment: { id: string; name: string }, status: '启用' | '停用') => {
    const environmentRow = target.getByRole('row').filter({ hasText: environment.name })
    await environmentRow.getByRole('button', { name: '编辑', exact: true }).click()
    const edit = target.getByRole('dialog', { name: '编辑环境', exact: true })
    await edit.getByRole('combobox', { name: '状态', exact: true }).click()
    await target.getByRole('option', { name: status, exact: true }).click()
    const saving = target.waitForResponse(result => result.url().endsWith(`/environments/${environment.id}`) && result.request().method() === 'PATCH')
    await edit.getByRole('button', { name: '保存', exact: true }).click()
    const result = await saving
    expect(result.status(), await result.text()).toBe(200)
    await expect(edit).not.toBeVisible()
  }
  await page.goto('/environments')
  for (const environment of environments) await setEnvironmentStatus(page, environment, '停用')
  await page.goto(root)
  await publish.click()
  await expect(dialog.getByText('请先在环境管理中创建或启用环境，再返回这里发布。')).toBeVisible()
  await expect(environmentSelect).toBeDisabled()
  await expect(dialog.getByRole('button', { name: '发布到环境…', exact: true })).toBeDisabled()
  const managementPage = context.waitForEvent('page')
  await dialog.getByRole('link', { name: '管理环境', exact: true }).click()
  const manager = await managementPage
  await expect(manager).toHaveURL(/\/environments$/)
  for (const environment of environments) await setEnvironmentStatus(manager, environment, '启用')
  await manager.close()
  await page.bringToFront()
  await expect(dialog).toBeVisible()
  await dialog.getByRole('button', { name: '重新加载环境', exact: true }).click()
  await expect(environmentSelect).toBeEnabled()
  await expect(environmentSelect).toContainText('Development')
  await dialog.getByRole('button', { name: '取消', exact: true }).click()

  await page.route('**/api/v1/environments', route => route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ message: 'Environment service unavailable' }) }))
  await page.reload()
  await publish.click()
  await expect(dialog.getByText('环境加载失败，请重试。')).toBeVisible()
  await expect(dialog.getByRole('link', { name: '管理环境', exact: true })).toHaveCount(0)
  await page.unroute('**/api/v1/environments')
  await dialog.getByRole('button', { name: '重新加载环境', exact: true }).click()
  await expect(environmentSelect).toBeEnabled()
  await expect(environmentSelect).toContainText('Development')
})
