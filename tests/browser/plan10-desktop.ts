import { expect, type Browser, type TestInfo } from '@playwright/test'
import { businessFrame, loginAt } from './plan9-page-support'
import { evidence, type BatchScenario } from './plan10-authoring'

/** Explicit saved preferences take precedence over the fixture user's profile defaults. */
export async function desktopMatrix(browser:Browser,scenario:BatchScenario,suffix:string,info:TestInfo,attempt:number){
  const profiles:Array<{locale:string;theme:string;modeLabel:string|null;retained:boolean}>=[]
  for(const locale of ['zh-CN','en-US'])for(const theme of ['light','dark']as const){
    const context=await browser.newContext({baseURL:process.env.AGENTX_E2E_BASE_URL,viewport:{width:1440,height:900},locale,colorScheme:theme})
    await context.addInitScript(({locale,theme})=>{
      if(window!==window.top)return
      localStorage.setItem('agentx.locale',locale)
      localStorage.setItem('agentx.theme',theme)
    },{locale,theme})
    const page=await context.newPage(),writes:string[]=[]
    page.on('request',request=>{if(request.method()==='POST'&&/\/pages\/[^/]+\/(batches|invocations)$/.test(new URL(request.url()).pathname))writes.push(request.url())})
    try{
      await loginAt(page,`/a/${suffix}`,process.env.AGENTX_E2E_PLAN9_EMPLOYEE!,process.env.AGENTX_E2E_PLAN9_EMPLOYEE_PASSWORD!)
      await expect(page.locator('html')).toHaveAttribute('lang',locale)
      await expect(page.locator('html')).toHaveAttribute('data-theme',theme)
      const frame=await businessFrame(page),mode=frame.locator('[data-action="batch-mode"]')
      await expect(mode).toHaveText(locale==='en-US'?/batch|bulk/i:/批量|批处理/)
      const modeLabel=await mode.textContent()
      await mode.click()
      const rows=frame.locator('[data-batch-row]')
      if(!await rows.count())await frame.locator('[data-action="add-batch-item"]').click()
      if(locale==='en-US')expect(await rows.first().innerText(),'English batch row titles, field labels and row actions must use the selected locale').not.toMatch(/\p{Script=Han}/u)
      if(locale==='en-US')expect((await frame.locator('label:visible,button:visible').allTextContents()).join(' '),'English form labels and action captions must use the selected locale').not.toMatch(/\p{Script=Han}/u)
      for(const button of await frame.locator('button:visible').all())await expect(button,'Visible actions need an accessible name').toHaveAccessibleName(/\S/)
      const field=scenario.kind==='quote'?'/customer':'/request_id'
      const editor=rows.first().locator(`[data-field-path="${field}"]`)
      const draft=`desktop-draft-${crypto.randomUUID().slice(0,8)}`
      await editor.fill(draft)
      await page.waitForTimeout(1500)
      await expect(editor).toHaveValue(draft)
      expect(await page.evaluate(()=>document.documentElement.scrollWidth>window.innerWidth)).toBe(false)
      expect(await frame.evaluate(()=>document.documentElement.scrollWidth>window.innerWidth+1)).toBe(false)
      expect(writes,'Changing a desktop preference and editing a draft cannot submit business work').toEqual([])
      await page.screenshot({path:info.outputPath(`${scenario.kind}-desktop-${attempt}-${locale}-${theme}.png`),fullPage:true,animations:'disabled'})
      profiles.push({locale,theme,modeLabel,retained:true})
      evidence(`${scenario.kind}-desktop-${attempt}`,{profiles})
    }catch(error){
      await page.screenshot({path:info.outputPath(`${scenario.kind}-desktop-${attempt}-${locale}-${theme}-failed.png`),fullPage:true,animations:'disabled'}).catch(()=>{})
      evidence(`${scenario.kind}-desktop-${attempt}`,{passed:false,profiles,locale,theme,error:error instanceof Error?error.message:String(error)})
      throw error
    }finally{await context.close()}
  }
  evidence(`${scenario.kind}-desktop-${attempt}`,{passed:true,profiles})
}
