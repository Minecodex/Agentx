import { test,expect } from '@playwright/test'
import { writeFileSync } from 'node:fs'
import { login } from '../studio-builder-support'
import { loginAt } from '../plan9-page-support'
import { useRuntimePortForward } from './playground-helpers'

type Request={phase:'register'|'authorize'|'revoke';applicationId:string;name:string;callback?:string;authorizationUrl?:string;nativeCallback?:boolean}
const input=JSON.parse(process.env.AGENTX_E2E_PLAN9_OAUTH!) as Request

test(`Personal OAuth: ${input.phase} through the formal Agentx UI`,async({page},info)=>{
  await useRuntimePortForward(page)
  if(input.phase==='register'){
    await login(page)
    await page.goto(`/applications/${input.applicationId}`)
    await page.getByRole('tab',{name:'MCP 接入',exact:true}).click()
    const diagnostics=page.waitForResponse(response=>new URL(response.url()).pathname.endsWith('/mcp/diagnostics'))
    await page.getByTestId('application-mcp-diagnostics').getByRole('button',{name:'连接诊断',exact:true}).click()
    const checked=await diagnostics;expect(checked.ok(),await checked.text()).toBe(true)
    expect((await checked.json()).passed).toBe(true)
    await page.locator('details').filter({has:page.locator('summary').getByText('个人 OAuth 客户端',{exact:true})}).locator('summary').click()
    await page.getByRole('button',{name:'登记客户端',exact:true}).click()
    const form=page.getByRole('dialog',{name:'个人 OAuth 客户端',exact:true})
    await form.getByLabel('客户端名称').fill(input.name)
    await form.getByLabel('回调地址，每行一个').fill(input.callback!)
    const registered=page.waitForResponse(response=>response.url().endsWith('/mcp/clients')&&response.request().method()==='POST')
    await form.getByRole('button',{name:'登记客户端',exact:true}).click()
    const response=await registered;expect(response.ok(),await response.text()).toBe(true)
    writeFileSync(process.env.AGENTX_E2E_PLAN9_OAUTH_REPORT!,JSON.stringify({client:await response.json(),diagnostics:await checked.json()},null,2))
  }else if(input.phase==='authorize'){
    await loginAt(page,input.authorizationUrl!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!)
    await expect(page.getByRole('heading',{name:'连接 Agentx 账号',exact:true})).toBeVisible()
    await page.getByRole('button',{name:'继续登录',exact:true}).click()
    await expect(page.getByRole('heading',{name:'允许客户端使用应用',exact:true})).toBeVisible()
    await page.screenshot({path:info.outputPath('personal-consent.png'),fullPage:true})
    await page.getByRole('button',{name:'允许连接',exact:true}).click()
    await expect(page).toHaveURL(url=>url.origin+url.pathname===input.callback)
    if(!input.nativeCallback)await expect(page.getByText('已收到授权结果，请返回客户端。',{exact:true})).toBeVisible()
  }else{
    await loginAt(page,'/oauth/grants',process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!)
    const grant=page.getByText(input.name,{exact:true}).locator('../..')
    await expect(grant).toBeVisible()
    const revoked=page.waitForResponse(response=>/\/oauth\/grants\/[^/]+$/.test(response.url())&&response.request().method()==='DELETE')
    await grant.getByRole('button',{name:'撤销',exact:true}).click()
    const response=await revoked;expect(response.ok(),await response.text()).toBe(true)
    await expect(page.getByText(input.name,{exact:true})).toHaveCount(0)
  }
})
