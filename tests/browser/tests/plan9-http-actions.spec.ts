import {expect,test,type Page} from '@playwright/test'
import {clickForResponse} from '../browser-http'

async function validSubmit(page:Page){
  page.setDefaultTimeout(2_000)
  await page.locator('body').evaluate(body=>{body.innerHTML='<button data-action="invoke">Submit</button>';body.querySelector('button')!.onclick=()=>{void fetch('/submit',{method:'POST'})}})
  const response=await clickForResponse(page,page.locator('[data-action="invoke"]'),response=>response.url().endsWith('/submit')&&response.request().method()==='POST')
  expect(response.ok()).toBe(true)
}
for(const kind of ['missing','disabled'])test(`A ${kind} business action fails normally and the next repair remains executable`,async({page})=>{
  await page.route('https://actions.agentx.test/**',route=>route.fulfill({status:200,contentType:'text/html',body:route.request().method()==='POST'?'Accepted':'<body></body>'}))
  await page.goto('https://actions.agentx.test/')
  page.setDefaultTimeout(150)
  if(kind==='disabled')await page.locator('body').evaluate(body=>{body.innerHTML='<button data-action="invoke" disabled>Unavailable</button>'})
  await expect(clickForResponse(page,page.locator('[data-action="invoke"]'),response=>response.url().endsWith('/submit'))).rejects.toThrow(/Timeout|timeout/)
  await validSubmit(page)
  await page.waitForTimeout(200)
})
