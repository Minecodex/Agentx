import { expect, test, type Response } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { evidence, type BatchScenario } from '../plan10-authoring'
import { login, read } from '../studio-builder-support'
import { trackAccessToken } from '../browser-http'
import { useRuntimePortForward } from './playground-helpers'

for(const kind of ['expense','quote','file'])test(`${kind}: the checks tab and visible preview assess the same saved AI page`,async({page})=>{
  test.setTimeout(180_000)
  const scenarios=JSON.parse(readFileSync(process.env.AGENTX_E2E_PLAN10_SCENARIOS!,'utf8')) as BatchScenario[]
  const scene=scenarios.find(value=>value.kind===kind)!
  await useRuntimePortForward(page)
  const token=trackAccessToken(page,await login(page))
  await page.goto(`/applications/${scene.applicationId}`)
  const opening=page.waitForResponse(response=>response.url().endsWith('/builder-sessions')&&response.request().method()==='POST')
  await page.getByRole('tab',{name:'访问界面',exact:true}).click()
  const binding=await (await opening).json() as {uiDraftId:string}
  const draft=await read<{htmlHash:string}>(page,await token(),`/applications/${scene.applicationId}/ui/drafts/${binding.uiDraftId}`)
  const report=()=>page.waitForResponse(response=>response.url().endsWith(`/ui/drafts/${binding.uiDraftId}/checks`)&&response.request().method()==='POST'&&response.request().postDataJSON().mode==='preview',{timeout:60_000})
  const payload=async(response:Response)=>{expect(response.ok(),await response.text()).toBe(true);return response.request().postDataJSON() as {htmlHash:string;errors:Array<{code:string;message:string}>}}
  await page.getByRole('tab',{name:'检查结果',exact:true}).click()
  const hidden=report()
  await page.getByRole('button',{name:'重新检查',exact:true}).click()
  const fromChecks=await payload(await hidden)
  const visible=report()
  await page.getByRole('button',{name:'重新检查',exact:true}).click()
  await page.getByRole('tab',{name:'模拟预览',exact:true}).click()
  const fromPreview=await payload(await visible)
  expect(fromChecks.htmlHash).toBe(draft.htmlHash);expect(fromPreview.htmlHash).toBe(draft.htmlHash)
  const codes=(value:typeof fromChecks)=>[...new Set(value.errors.map(error=>error.code))].sort()
  evidence(`E10-U07-${kind}-check-visibility`,{sourceHash:draft.htmlHash,fromChecks,fromPreview,passed:JSON.stringify(codes(fromChecks))===JSON.stringify(codes(fromPreview))})
  expect(codes(fromChecks)).toEqual(codes(fromPreview))
})
