import { expect, type Page, type Frame } from '@playwright/test'
import { businessFrame } from './plan9-page-support'
import { clickForResponse, trackAccessToken } from './browser-http'
import { evidence } from './plan10-authoring'

type Token=ReturnType<typeof trackAccessToken>
const mapping:Record<string,string>={'/customer':'客户列','/quantity':'数量列','/unitPrice':'单价列','/discountPercent':'折扣列'}
async function columns(frame:Frame){
  for(const[field,header]of Object.entries(mapping)){
    const select=frame.locator(`select[data-column-field="${field}"]`)
    await expect(select).toBeVisible()
    const value=await select.locator('option').evaluateAll((options,header)=>{
      const option=options.find(option=>option.textContent?.trim()===header)??options.find(option=>option.textContent?.includes(header))
      return(option as HTMLOptionElement|undefined)?.value
    },header)
    expect(value,`Explicit mapping for ${field}`).toBeDefined()
    await select.selectOption(value!)
  }
}
function parsed(page:Page){return page.waitForResponse(response=>response.url().endsWith('/batch-imports/parse')&&response.request().method()==='POST')}

/** Table parsing and business submission use distinct real endpoints and explicit UI actions. */
export async function importedQuoteBatch(page:Page,token:Token,suffix:string,attempt:number){
  const root=process.env.AGENTX_E2E_RUNTIME_URL!
  await page.goto(`/a/${suffix}`)
  let frame=await businessFrame(page)
  await frame.locator('[data-action="batch-mode"]').click()
  const posts:string[]=[]
  let accepted:string|undefined
  const listener=(request:import('@playwright/test').Request)=>{if(request.method()==='POST'&&/\/pages\/[^/]+\/(batches|invocations)$/.test(new URL(request.url()).pathname))posts.push(request.url())}
  page.on('request',listener)
  try{
    const marker=crypto.randomUUID().slice(0,8)
    const customers=[`CSV客户甲-${marker}`,`CSV客户乙,分公司-${marker}`,`CSV客户丙-${marker}`]
    const text=`折扣列,数量列,客户列,单价列\n5,12,${customers[0]},25.75\n7.5,19,"${customers[1]}",44.5\n0,21,${customers[2]},88.25\n`
    await frame.locator('[data-import-text]').fill(text)
    const parsing=parsed(page)
    await frame.locator('[data-action="parse-table"]').click()
    const csv=await parsing;expect(csv.ok(),await csv.text()).toBe(true)
    const table=await csv.json() as {headers:string[];rows:string[][];lines:number[]}
    expect(table.rows).toHaveLength(3);expect(table.lines).toEqual([2,3,4])
    await columns(frame)
    await frame.locator('[data-action="apply-table"]').click()
    const rows=frame.locator('[data-batch-row]')
    await expect(rows).toHaveCount(3)
    expect(posts,'Parsing and applying a mapping cannot create business work').toEqual([])
    for(let index=0;index<3;index++)await expect(rows.nth(index).locator('[data-field-path="/customer"]')).toHaveValue(customers[index]!)
    const quantity=rows.last().locator('[data-field-path="/quantity"]')
    await quantity.fill('0')
    expect(await quantity.evaluate(element=>(element as HTMLInputElement).checkValidity()),'Imported rows retain the application numeric constraints').toBe(false)
    expect(posts).toEqual([])
    await quantity.fill('23')
    await page.waitForTimeout(2200)
    await expect(quantity).toHaveValue('23')
    const receipt=await clickForResponse(page,frame.locator('[data-action="create-batch"]'),response=>response.url().endsWith(`/pages/${suffix}/batches`)&&response.request().method()==='POST')
    expect(receipt.status(),await receipt.text()).toBe(202)
    const batch=await receipt.json() as {batchId:string;counts:{total:number}}
    accepted=batch.batchId
    evidence(`quote-import-${attempt}`,{stage:'accepted',batch,csv:table,mapping})
    expect(batch.counts.total).toBe(3)
    let projects:Array<{input:Record<string,unknown>;task:{state:string;outputs:Record<string,unknown>}|null}>=[]
    await expect.poll(async()=>{
      const response=await page.request.get(`${root}/gateway/v1/batches/${batch.batchId}/items`,{headers:{Authorization:`Bearer ${await token()}`}})
      expect(response.ok(),await response.text()).toBe(true);projects=(await response.json()).items
      if(projects.some(item=>item.task&&['failed','outcome_unknown'].includes(item.task.state)))throw new Error('FORMAL_BUSINESS_FAILURE: imported inputs reached a failed or uncertain execution')
      return projects.length===3&&projects.every(item=>item.task?.state==='completed')
    },{timeout:360_000,intervals:[1000,2000]}).toBe(true)
    expect(projects.map(item=>item.input)).toEqual([
      {customer:customers[0],quantity:12,unitPrice:25.75,discountPercent:5},
      {customer:customers[1],quantity:19,unitPrice:44.5,discountPercent:7.5},
      {customer:customers[2],quantity:23,unitPrice:88.25,discountPercent:0},
    ])
    expect(posts).toHaveLength(1)
    // A TSV upload is also preparation and must send an owned Artifact to the Rust parser.
    await page.goto(`/a/${suffix}`);frame=await businessFrame(page)
    await frame.locator('[data-action="batch-mode"]').click()
    const selects=frame.locator('select')
    for(let index=0;index<await selects.count();index++){
      const select=selects.nth(index)
      if(!await select.isVisible()||await select.getAttribute('data-column-field'))continue
      const delimiter=await select.locator('option').evaluateAll(options=>(options.find(option=>/TSV|Tab|制表/i.test(option.textContent??''))as HTMLOptionElement|undefined)?.value)
      if(delimiter!==undefined)await select.selectOption(delimiter)
    }
    let didParse=false
    const tsvResponse=parsed(page).then(response=>{didParse=true;return response})
    await frame.locator('input[data-action="import-file"]').setInputFiles({name:'正式报价导入.tsv',mimeType:'text/tab-separated-values',buffer:Buffer.from(`客户列\t数量列\t单价列\t折扣列\nTSV-${marker}\t7\t31.5\t2.5\n`)})
    await page.waitForTimeout(700)
    if(!didParse)await frame.locator('[data-action="parse-table"]').click()
    const tsv=await tsvResponse;expect(tsv.ok(),await tsv.text()).toBe(true)
    expect(tsv.request().postDataJSON()).toMatchObject({delimiter:'\t'})
    expect(tsv.request().postDataJSON().artifactId).toMatch(/^[0-9a-f-]{36}$/)
    await columns(frame);await frame.locator('[data-action="apply-table"]').click()
    await expect(frame.locator('[data-batch-row]')).toHaveCount(1)
    await expect(frame.locator('[data-batch-row] [data-field-path="/customer"]')).toHaveValue(`TSV-${marker}`)
    expect(posts).toHaveLength(1)
    const proof={batch,projects,csv:table,tsv:await tsv.json(),mapping,editedQuantity:23,formalRequests:posts}
    evidence(`quote-import-${attempt}`,{passed:true,...proof});return proof
  }catch(error){
    evidence(`quote-import-${attempt}`,{passed:false,batchId:accepted,error:error instanceof Error?error.message:String(error)})
    if(accepted){
      const response=await page.request.post(`${root}/gateway/v1/batches/${accepted}/cancel`,{headers:{Authorization:`Bearer ${await token()}`,'Idempotency-Key':`validation-cleanup:${accepted}`}})
      expect(response.ok(),await response.text()).toBe(true)
    }
    throw error
  }finally{page.off('request',listener)}
}
