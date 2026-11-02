import {expect,test} from '@playwright/test'
import {writeFileSync} from 'node:fs'
import {login} from '../studio-builder-support'
import {loginAt} from '../plan9-page-support'
import {useRuntimePortForward} from './playground-helpers'

const data=JSON.parse(process.env.AGENTX_E2E_PLAN9_CATALOG!) as {name:string;suffix:string;phase:string;report:string}
const observations:unknown[]=[]
for(const actor of ['administrator','reader'])test(`${actor}: directory opens and copies only the active page during ${data.phase}`,async({page,context},info)=>{
  await context.grantPermissions(['clipboard-read','clipboard-write'])
  await useRuntimePortForward(page)
  if(actor==='administrator'){await login(page);await page.goto('/applications')}
  else await loginAt(page,'/applications',process.env.AGENTX_E2E_PLAN9_CATALOG_USER!,process.env.AGENTX_E2E_PLAN9_CATALOG_PASSWORD!)
  await page.getByRole('searchbox',{name:'搜索应用',exact:true}).fill(data.name)
  const row=page.getByRole('row').filter({hasText:data.name})
  await expect(row).toHaveCount(1)
  const link=row.locator('a[href^="/a/"]')
  await expect(link).toHaveAttribute('href',`/a/${data.suffix}`)
  await row.getByRole('button',{name:'复制访问地址',exact:true}).click()
  const copied=await page.evaluate(()=>navigator.clipboard.readText())
  expect(copied).toBe(new URL(`/a/${data.suffix}`,process.env.AGENTX_E2E_BASE_URL).href)
  const shell=await page.request.get(`/a/${data.suffix}`)
  expect(shell.ok()).toBe(true)
  expect(shell.headers()['cross-origin-opener-policy']).toBe('same-origin')
  expect(shell.headers()['x-frame-options']).toBe('DENY')
  await page.screenshot({path:info.outputPath(`catalog-${data.phase}-${actor}.png`),fullPage:true})
  observations.push({actor,phase:data.phase,href:await link.getAttribute('href'),copied,
    shellHeaders:{coop:shell.headers()['cross-origin-opener-policy'],xFrameOptions:shell.headers()['x-frame-options']}})
  writeFileSync(data.report,JSON.stringify(observations,null,2))
})
