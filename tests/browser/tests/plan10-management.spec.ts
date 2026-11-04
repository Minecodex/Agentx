import { expect, test, type Page } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { login, read } from '../studio-builder-support'
import { clickForResponse, trackAccessToken } from '../browser-http'
import { useRuntimePortForward } from './playground-helpers'

type Batch={batchId:string;name:string;callerId:string;callerType:string;sourceBatchId:string|null;counts:{total:number;failed:number;cancelled:number}}
const input=process.env.AGENTX_E2E_PLAN10_MANAGEMENT?JSON.parse(readFileSync(process.env.AGENTX_E2E_PLAN10_MANAGEMENT,'utf8')) as {applicationId:string;stop:Batch;cancel:Batch;failed:Batch;managerId:string}:undefined
async function detail(page:Page,id:string){
  await page.goto(`/applications/${input!.applicationId}?tab=invocations&recordTab=batch&batchId=${id}`)
  await expect(page.getByTestId('application-batch-detail')).toContainText(id)
  await expect(page.getByTestId('application-batch-detail').getByRole('checkbox').first()).toBeAttached()
}
async function confirm(page:Page,name:string,action:string){
  await page.getByTestId('application-batch-detail').getByRole('button',{name,exact:true}).click()
  const dialog=page.getByRole('dialog',{name,exact:true})
  const receipt=await clickForResponse(page,dialog.getByRole('button',{name:'确认',exact:true}),response=>response.url().endsWith(`/${action}`)&&response.request().method()==='POST')
  expect(receipt.ok(),await receipt.text()).toBe(true);return receipt.json() as Promise<Batch>
}

test('stop and cancel remain distinct in the original application call records',async({page},info)=>{
  test.setTimeout(180_000);expect(input).toBeTruthy()
  await useRuntimePortForward(page);await login(page)
  await page.goto(`/applications/${input!.applicationId}`)
  await page.getByRole('tab',{name:'调用记录',exact:true}).click()
  await expect(page.getByRole('tab',{name:'单次调用',exact:true})).toBeVisible()
  await page.getByRole('tab',{name:'批次记录',exact:true}).click()
  await expect(page.getByTestId('application-batches')).toContainText(input!.stop.name)
  await detail(page,input!.stop.batchId)
  await expect(page.getByRole('button',{name:'停止后续派发',exact:true})).toBeEnabled()
  await expect(page.getByTestId('application-batch-detail')).not.toContainText('applicationBatches.')
  await page.getByRole('button',{name:'停止后续派发',exact:true}).click()
  const stopDialog=page.getByRole('dialog',{name:'停止后续派发',exact:true})
  await expect(stopDialog).toContainText('已受理项目继续运行')
  const stopped=await clickForResponse(page,stopDialog.getByRole('button',{name:'确认',exact:true}),response=>response.url().endsWith('/stop')&&response.request().method()==='POST')
  expect((await stopped.json() as Batch).counts.cancelled).toBe(3)
  await expect(page.getByRole('button',{name:'停止后续派发',exact:true})).toBeDisabled()
  await detail(page,input!.cancel.batchId)
  const cancelled=await confirm(page,'取消未结束项','cancel')
  expect(cancelled.counts.cancelled).toBe(3)
  await page.screenshot({path:info.outputPath('original-call-records-actions.png'),fullPage:true})
})

test('desktop themes and safe rerun keep original data and manager identity',async({page},info)=>{
  test.setTimeout(300_000);expect(input).toBeTruthy()
  await useRuntimePortForward(page);const token=trackAccessToken(page,await login(page))
  await detail(page,input!.failed.batchId)
  for(const[language,english]of [['English',true],['简体中文',false]]as const){
    await page.getByRole('button',{name:/^(语言|Language)$/}).click()
    await page.getByRole('menuitemradio',{name:language,exact:true}).click()
    for(const dark of [true,false]){
      await page.getByRole('button',{name:/^(主题|Theme)$/}).click()
      await page.getByRole('menuitemradio',{name:english?dark?'Dark':'Light':dark?'深色':'浅色',exact:true}).click()
      await expect(page.getByTestId('application-batch-detail')).not.toContainText('applicationBatches.')
      const overflow=await page.evaluate(()=>document.documentElement.scrollWidth>window.innerWidth)
      expect(overflow).toBe(false)
      await page.screenshot({path:info.outputPath(`batches-${english?'en':'zh'}-${dark?'dark':'light'}.png`),fullPage:true,animations:'disabled'})
    }
  }
  await page.getByRole('checkbox',{name:'选择失败项 1',exact:true}).check()
  await page.getByRole('button',{name:'重新执行选中失败项',exact:true}).click()
  const dialog=page.getByRole('dialog',{name:'重新执行选中失败项',exact:true})
  const receipts:Array<{key:string|null;batch:Batch}>=[]
  const pattern=`**/api/v1/applications/${input!.applicationId}/batches/${input!.failed.batchId}/rerun`
  await page.route(pattern,async route=>{
    const response=await route.fetch(),batch=await response.json() as Batch
    receipts.push({key:route.request().headers()['idempotency-key']??null,batch})
    if(receipts.length===1)await route.abort('failed');else await route.fulfill({response})
  })
  await dialog.getByRole('button',{name:'确认',exact:true}).click()
  await expect.poll(()=>receipts.length).toBe(1)
  await expect(dialog.getByRole('button',{name:'确认',exact:true})).toBeEnabled()
  await dialog.getByRole('button',{name:'确认',exact:true}).click()
  await expect.poll(()=>receipts.length).toBe(2)
  expect(receipts[0]!.key).toBe(receipts[1]!.key)
  expect(receipts[0]!.batch.batchId).toBe(receipts[1]!.batch.batchId)
  const derived=receipts[1]!.batch
  expect(derived.callerType).toBe('user');expect(derived.callerId).toBe(input!.managerId)
  expect(derived.sourceBatchId).toBe(input!.failed.batchId)
  await expect(page).toHaveURL(new RegExp(`batchId=${derived.batchId}`))
  await expect(page.getByRole('button',{name:'来源批次',exact:true})).toBeVisible()
  await confirm(page,'取消未结束项','cancel')
  await expect.poll(async()=>(await read<Batch>(page,await token(),`/applications/${input!.applicationId}/batches/${derived.batchId}`)).counts.cancelled,{timeout:120_000}).toBe(1)
  await page.getByRole('button',{name:'导出汇总',exact:true}).click()
  const downloading=page.waitForEvent('download')
  await page.getByRole('button',{name:'下载 CSV',exact:true}).click()
  expect((await downloading).suggestedFilename()).toContain(derived.batchId)
  const original=await read<Batch>(page,await token(),`/applications/${input!.applicationId}/batches/${input!.failed.batchId}`)
  expect(original.counts.failed).toBe(3)
})
