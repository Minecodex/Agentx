import {expect,test,type Page} from '@playwright/test'
import {existsSync,readFileSync,writeFileSync} from 'node:fs'
import {businessFrame,fillFields,loginAt,type Scenario,type Task} from '../plan9-page-support'
import {trackAccessToken} from '../browser-http'
import {useRuntimePortForward} from './playground-helpers'
const data=JSON.parse(process.env.AGENTX_E2E_PLAN9_APPEARANCE!) as {scenario:Scenario;suffix:string;report:string}
const result:{appearance:unknown[];accountSwitch?:unknown;authRecovery?:unknown}=existsSync(data.report)?JSON.parse(readFileSync(data.report,'utf8')):{appearance:[]}
async function signIn(page:Page,locale:string){
  await useRuntimePortForward(page);await page.goto(`/a/${data.suffix}`)
  await page.getByLabel(locale==='en-US'?'Username':'用户名',{exact:true}).fill(process.env.AGENTX_E2E_PLAN9_EMPLOYEE!)
  await page.getByLabel(locale==='en-US'?'Password':'密码',{exact:true}).fill(process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!)
  await page.getByRole('button',{name:locale==='en-US'?'Sign in':'登录',exact:true}).click()
  await expect(page.getByTestId('application-business-shell')).toBeVisible()
}

test('The real AI page inherits desktop platform themes, language and keyboard behavior',async({browser},info)=>{
  test.setTimeout(300_000)
  try{
    for(const locale of ['zh-CN','en-US'])for(const theme of ['light','dark']){
      const context=await browser.newContext({baseURL:process.env.AGENTX_E2E_BASE_URL,locale,viewport:{width:1440,height:900}})
      try{
        await context.addInitScript(({locale,theme})=>{if(window.top===window){localStorage.setItem('agentx.locale',locale);localStorage.setItem('agentx.theme',theme)}},{locale,theme})
        const page=await context.newPage();await signIn(page,locale)
        await expect(page.locator('html')).toHaveAttribute('data-theme',theme)
        await expect(page.getByTestId('application-new-task')).toContainText(locale==='en-US'?'New task':'新建任务')
        const frame=await businessFrame(page)
        const host=await page.evaluate(()=>getComputedStyle(document.documentElement).getPropertyValue('--ui-background').trim())
        expect(host).not.toBe('')
        expect(await frame.evaluate(()=>getComputedStyle(document.documentElement).getPropertyValue('--background').trim())).toBe(host)
        const first=frame.locator('input:not([type=hidden])').first()
        await first.focus();await expect(first).toBeFocused();await first.press('Tab')
        expect(await frame.evaluate(()=>document.activeElement?.tagName)).toMatch(/INPUT|SELECT|TEXTAREA|BUTTON/)
        expect(await frame.evaluate(()=>{const api=(window as Window&{agentx?:{token?:unknown;apiKey?:unknown;approve?:unknown;request?:unknown}}).agentx;return Boolean(api?.token||api?.apiKey||api?.approve||api?.request)})).toBe(false)
        await page.screenshot({path:info.outputPath(`${locale}-${theme}.png`),fullPage:true})
        result.appearance.push({locale,theme,platformBackground:host,privateCapabilities:true,keyboard:true})
      }finally{await context.close()}
    }
  }finally{writeFileSync(data.report,JSON.stringify(result,null,2))}
})

test('Logout destroys the old page and a reviewer cannot inherit the employee task',async({page},info)=>{
  test.setTimeout(180_000)
  const root=process.env.AGENTX_E2E_RUNTIME_URL
  const token=await loginAt(page,`/a/${data.suffix}`,process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!)
  const frame=await businessFrame(page)
  await fillFields(frame,data.scenario.inputs[0])
  const sent=page.waitForResponse(response=>response.url().endsWith(`/pages/${data.suffix}/invocations`)&&response.request().method()==='POST')
  await frame.locator('[data-action="invoke"]').click()
  const accepted=await sent;expect(accepted.ok(),await accepted.text()).toBe(true)
  const id=(await accepted.json()).taskId as string
  let task:Task|undefined
  await expect.poll(async()=>{const response=await page.request.get(`${root}/gateway/v1/tasks/${id}`,{headers:{Authorization:`Bearer ${token}`}});expect(response.ok()).toBe(true);task=await response.json();return task!.state},{timeout:60_000}).toBe('completed')
  await expect(frame.locator('body')).toContainText(String(data.scenario.expected[0]!.customer))
  await page.getByRole('button',{name:'退出登录',exact:true}).click()
  await expect(page.locator('iframe')).toHaveCount(0)
  expect(frame.isDetached()).toBe(true)
  const reviewer=(JSON.parse(process.env.AGENTX_E2E_PLAN9_REVIEWERS!) as Record<string,{username:string;password:string}>).finance!
  const finance=await loginAt(page,`/a/${data.suffix}`,reviewer.username,reviewer.password)
  await expect(page.locator('iframe')).toHaveCount(0)
  const reviewerIdentity=await page.request.get('/api/v1/auth/me',{headers:{Authorization:`Bearer ${finance}`}})
  expect(reviewerIdentity.ok(),await reviewerIdentity.text()).toBe(true)
  const denied=await page.request.get(`${root}/gateway/v1/tasks/${id}`,{headers:{Authorization:`Bearer ${finance}`}})
  expect([401,403,404]).toContain(denied.status())
  await page.screenshot({path:info.outputPath('reviewer-cannot-inherit-employee-task.png'),fullPage:true})
  result.accountSwitch={employeeTaskId:id,detachedOriginalFrame:true,reviewerCannotReadStatus:denied.status()}
  writeFileSync(data.report,JSON.stringify(result,null,2))
})

test('An actual authorization failure refreshes the same employee session and preserves the submitted task',async({page},info)=>{
  test.setTimeout(180_000)
  const token=await loginAt(page,`/a/${data.suffix}`,process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!)
  const currentToken=trackAccessToken(page,token)
  const taskStatuses:number[]=[],refreshStatuses:number[]=[],acceptedTasks:string[]=[]
  let injected=false
  page.on('response',response=>{
    if(/\/gateway\/v1\/tasks\//.test(response.url()))taskStatuses.push(response.status())
    if(response.url().endsWith('/api/v1/auth/refresh'))refreshStatuses.push(response.status())
    if(response.url().endsWith(`/pages/${data.suffix}/invocations`)&&response.request().method()==='POST'&&response.ok())void response.json().then(task=>acceptedTasks.push(task.taskId))
  })
  await page.route('**/gateway/v1/tasks/*',async route=>{
    if(injected||route.request().method()!=='GET'){await route.continue();return}
    injected=true
    // The real Gateway rejects the invalid credential; no synthetic 401 response.
    await route.continue({headers:{...route.request().headers(),authorization:'Bearer expired-session-test-credential'}})
  })
  let frame=await businessFrame(page)
  await fillFields(frame,data.scenario.inputs[0])
  const sent=page.waitForResponse(response=>response.url().endsWith(`/pages/${data.suffix}/invocations`)&&response.request().method()==='POST')
  await frame.locator('[data-action="invoke"]').click()
  const created=await sent;expect(created.ok(),await created.text()).toBe(true)
  const id=(await created.json()).taskId as string
  await expect.poll(()=>refreshStatuses,{timeout:60_000}).toContain(200)
  await expect.poll(()=>taskStatuses,{timeout:60_000}).toContain(401)
  await expect.poll(()=>taskStatuses,{timeout:60_000}).toContain(200)
  await expect(frame.locator('body')).toContainText(String(data.scenario.expected[0]!.customer))
  await expect(page.getByTestId('application-current-task')).toHaveText(id)
  expect(acceptedTasks).toEqual([id])
  let task:Task|undefined
  await expect.poll(async()=>{
    expect(await currentToken()).not.toBe(token)
    const response=await page.request.get(`${process.env.AGENTX_E2E_RUNTIME_URL}/gateway/v1/tasks/${id}`,{headers:{Authorization:`Bearer ${await currentToken()}`}})
    expect(response.ok(),await response.text()).toBe(true);task=await response.json();return task!.state
  },{timeout:60_000}).toBe('completed')
  expect(task!.outputs).toEqual(data.scenario.expected[0])
  await page.reload();await expect(page.getByTestId('application-current-task')).toHaveText(id)
  frame=await businessFrame(page);await expect(frame.locator('body')).toContainText(String(task!.outputs.customer))
  expect(acceptedTasks).toEqual([id])
  await page.screenshot({path:info.outputPath('same-employee-session-restored.png'),fullPage:true})
  result.authRecovery={taskId:id,taskStatuses,refreshStatuses,acceptedTasks,restoredSameTask:true,evidenceUsesRefreshedCredential:true}
  writeFileSync(data.report,JSON.stringify(result,null,2))
})
