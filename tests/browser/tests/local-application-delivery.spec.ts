import { expect, test } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const path=process.env.AGENTX_LOCAL_APPLICATION_DELIVERY_CONFIG
test.skip(!path,'Requires an explicitly retained local application')
// Retained account credentials stay out of browser trace and video artifacts.
test.use({trace:'off',video:'off'})

test('The local approval center approves the pending MCP task',async({page,context},info)=>{
  const config=JSON.parse(readFileSync(path!,'utf8')) as {baseUrl:string;credentialsFile:string;evidenceDir:string}
  const credentials=JSON.parse(readFileSync(config.credentialsFile,'utf8')) as {username:string;password:string}
  const task=JSON.parse(readFileSync(join(config.evidenceDir,'mcp-business-test.json'),'utf8')) as {phase:string;approvalId:string;taskId:string}
  expect(task.phase).toBe('waiting_for_ui_approval')
  await context.route(/^https:\/\/(?:run\.)?agentx\.localhost\//,async route=>{
    const response=await route.fetch();await route.fulfill({response})
  })
  await page.goto(config.baseUrl+'/login')
  await page.getByLabel('用户名',{exact:true}).fill(credentials.username)
  await page.getByLabel('密码',{exact:true}).fill(credentials.password)
  const logged=page.waitForResponse(response=>response.url().endsWith('/api/v1/auth/login')&&response.request().method()==='POST')
  await page.getByRole('button',{name:'登录',exact:true}).click()
  expect((await logged).ok()).toBe(true)
  const loaded=page.waitForResponse(response=>response.url().endsWith('/api/v1/approvals/'+task.approvalId)&&response.request().method()==='GET')
  await page.goto(config.baseUrl+'/approvals/'+task.approvalId)
  const view=await loaded;expect(view.ok()).toBe(true)
  const approval=(await view.json()) as {buttons:Array<{id:string;label:string}>}
  const label=approval.buttons.find(button=>button.id==='approved')!.label
  const claim=page.waitForResponse(response=>response.url().endsWith('/approvals/'+task.approvalId+'/claim')&&response.request().method()==='POST')
  await page.getByRole('button',{name:'领取',exact:true}).click()
  expect((await claim).ok()).toBe(true)
  await page.getByRole('button',{name:label,exact:true}).click()
  const dialog=page.getByRole('dialog',{name:label,exact:true})
  await dialog.getByLabel('理由（可选）').fill('统一升级后实际界面验收，虚构报销数据')
  const decide=page.waitForResponse(response=>response.url().endsWith('/approvals/'+task.approvalId+'/decide')&&response.request().method()==='POST')
  await dialog.getByRole('button',{name:label,exact:true}).click()
  const decision=await decide;expect(decision.ok(),await decision.text()).toBe(true)
  await expect(dialog).toHaveCount(0)
  await page.screenshot({path:info.outputPath('expense-mcp-approval.jpg'),type:'jpeg',fullPage:true})
})

test('The published local application opens with real login, its saved HTML and task history',async({page,context},info)=>{
  const config=JSON.parse(readFileSync(path!,'utf8')) as {baseUrl:string;credentialsFile:string;evidenceDir:string}
  const scenario=JSON.parse(readFileSync(join(config.evidenceDir,'scenario.json'),'utf8')) as {workflowId:string;versionId:string;applicationId:string}
  const credentials=JSON.parse(readFileSync(config.credentialsFile,'utf8')) as {username:string;password:string}
  // The API transport trusts the explicit local CA through NODE_EXTRA_CA_CERTS.
  // Every fulfilled response is fetched from the real HTTPS deployment; this
  // avoids installing a CA into the developer's system/browser trust stores.
  await context.route(/^https:\/\/(?:run\.)?agentx\.localhost\//,async route=>{
    const response=await route.fetch()
    await route.fulfill({response})
  })
  await page.goto(config.baseUrl+'/a/expense-demo')
  await page.getByLabel('用户名',{exact:true}).fill(credentials.username)
  await page.getByLabel('密码',{exact:true}).fill(credentials.password)
  const accepted=page.waitForResponse(response=>response.url().endsWith('/api/v1/auth/login')&&response.request().method()==='POST')
  await page.getByRole('button',{name:'登录',exact:true}).click()
  expect((await accepted).ok()).toBe(true)
  await expect(page).not.toHaveURL(/\/login(?:\?|$)/)
  await page.goto(config.baseUrl+'/a/expense-demo')
  await expect(page.getByTestId('application-business-shell')).toBeVisible()
  const business=page.frameLocator('iframe').frameLocator('iframe[name="agentx-application-page"]')
  for(const label of ['报销单号','申请人','费用类型','申请金额（元）'])await expect(business.getByLabel(label,{exact:true})).toBeVisible()
  await expect(business.getByRole('button',{name:'提交申请',exact:true})).toBeVisible()
  await expect(business.getByRole('button',{name:'刷新列表',exact:true})).toBeVisible()
  await expect(page.getByRole('alert')).toHaveCount(0)
  await page.screenshot({path:info.outputPath('expense-demo.jpg'),type:'jpeg',fullPage:true})
  await page.goto(`${config.baseUrl}/workflows/${scenario.workflowId}/versions/${scenario.versionId}`)
  for(const name of ['补充费用用途与发票号','经理审批报销'])await expect(page.getByText(name,{exact:true}).first()).toBeVisible()
  await page.screenshot({path:info.outputPath('expense-workflow.jpg'),type:'jpeg',fullPage:true})
  await page.goto(`${config.baseUrl}/applications/${scenario.applicationId}?tab=mcp`)
  await expect(page.getByRole('tab',{name:'MCP 接入',exact:true})).toHaveAttribute('data-state','active')
  await expect(page.getByTestId('mcp-documentation').getByRole('heading',{name:'submit_expense',exact:true})).toBeVisible()
  await page.screenshot({path:info.outputPath('expense-mcp.jpg'),type:'jpeg',fullPage:true})
  const identities=JSON.parse(readFileSync(join(config.evidenceDir,'web-ui-execution-ids.json'),'utf8')) as {approvedTaskId:string;rejectedTaskId:string}
  const mutations:string[]=[]
  page.on('request',request=>{if(request.method()==='POST'&&/\/pages\/[^/]+\/invocations|\/invocations\/[^/]+\/waits\//.test(new URL(request.url()).pathname))mutations.push(request.url())})
  for(const result of [
    {id:identities.approvedTaskId,applicant:'张小明',amount:'¥1,280.50',decision:'审批通过',file:'expense-approved.jpg'},
    {id:identities.rejectedTaskId,applicant:'李晓雨',amount:'¥680.00',decision:'审批拒绝',file:'expense-rejected.jpg'},
  ]){
    await page.goto(`${config.baseUrl}/a/expense-demo?taskId=${result.id}`)
    await expect(page.getByTestId('application-current-task')).toHaveText(result.id)
    const restored=page.frameLocator('iframe').frameLocator('iframe[name="agentx-application-page"]')
    for(const value of [result.applicant,result.amount,result.decision])await expect(restored.getByText(value,{exact:true}).first()).toBeVisible()
    await page.screenshot({path:info.outputPath(result.file),type:'jpeg',fullPage:true})
  }
  expect(mutations,'Opening completed tasks must not replay a business submission or input').toEqual([])
})
