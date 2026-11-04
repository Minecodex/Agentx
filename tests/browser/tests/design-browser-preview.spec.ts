import {expect,test,type Page} from '@playwright/test'
import {readFileSync,writeFileSync} from 'node:fs'
import {createEditorStore} from '../../../src/web/src/features/workflow-designer/store/editor-store'
import {serializeStudio} from '../../../src/web/src/features/workflow-designer/model/serializer'
import type {StudioNode} from '../../../src/web/src/features/workflow-designer/model/types'
import type {PageContract} from '../../../src/web/src/features/applications/ui/page-types'

type PreviewWindow=Window & {agentxDesignPreview:{ready:boolean;inspect:()=>Promise<unknown>;reload:()=>Promise<void>;facts:()=>{counts:{invoke:number;formalRequests:number};latestTask:{taskId:string}}}}
const assets=new URL('../../../.local/dist/design-preview/',import.meta.url)
const contract:PageContract={applicationId:'app',applicationName:'Observation',description:null,deploymentId:'deployment',workflowVersionId:'version',bundleId:'bundle',inputSchema:{type:'object',properties:{customer:{type:'string'}},required:['customer']},outputSchema:{type:'object',properties:{answer:{type:'string'}}},waitInputNodes:[],approvalStages:[],chatMapping:null,sdkVersion:2,maxUploadBytes:1048576}
const html=`<h1>Observation fixture</h1><label>Customer<input id="customer" data-field-path="/customer"></label><button id="send" data-action="invoke">Submit</button><output id="state"></output><output id="task"></output><script>
function render(t){document.getElementById('state').textContent=t.state;document.getElementById('task').textContent=t.taskId}
document.getElementById('send').onclick=async()=>{const t=await agentx.invoke({customer:document.getElementById('customer').value});render(t);await agentx.subscribeTask(t.taskId,render)};
agentx.getContext().then(async c=>{if(c.taskId){render(await agentx.getTask(c.taskId));await agentx.subscribeTask(c.taskId,render)}});
</script>`

async function mount(page:Page,document:unknown,locale:string,theme:string){
  const errors:string[]=[]
  page.on('pageerror',error=>errors.push(error.message))
  page.on('console',message=>{if(message.type()==='error')errors.push(message.text())})
  const input=JSON.stringify({document,options:{locale,theme}}).replaceAll('<','\\u003c')
  await page.route('https://preview.agentx.test/**',route=>{
    const path=new URL(route.request().url()).pathname
    if(path==='/')return route.fulfill({contentType:'text/html',body:`<!doctype html><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="worker-src blob:"><link rel="stylesheet" href="/preview.css"><div id="root"></div><script>window.agentxDesignInput=${input}</script><script src="/preview.js"></script>`})
    if(path==='/preview.js'||path==='/preview.css')return route.fulfill({contentType:path.endsWith('.js')?'text/javascript':'text/css',body:readFileSync(new URL(path.slice(1),assets))})
    return route.abort()
  })
  await page.goto('https://preview.agentx.test/')
  await page.waitForFunction(()=>(window as unknown as PreviewWindow).agentxDesignPreview?.ready)
  await page.evaluate(()=>(window as unknown as PreviewWindow).agentxDesignPreview.inspect())
  return errors
}

for(const locale of ['zh-CN','en-US'])for(const theme of ['light','dark'])test(`actual page renderer preserves its task after reload ${locale} ${theme}`,async({page})=>{
  const errors=await mount(page,{kind:'application_page',html,contract,contractHash:'fixture'},locale,theme)
  const frame=page.frameLocator('iframe').frameLocator('iframe')
  await frame.locator('#customer').fill('Fresh customer')
  await frame.locator('#send').click()
  await expect(frame.locator('#state')).toHaveText('completed')
  const id=await frame.locator('#task').innerText()
  await page.evaluate(()=>(window as unknown as PreviewWindow).agentxDesignPreview.reload())
  await page.evaluate(()=>(window as unknown as PreviewWindow).agentxDesignPreview.inspect())
  await expect(frame.locator('#state')).toHaveText('completed')
  await expect(frame.locator('#task')).toHaveText(id)
  const facts=await page.evaluate(()=>(window as unknown as PreviewWindow).agentxDesignPreview.facts())
  expect(facts.counts).toMatchObject({invoke:1,formalRequests:0})
  expect(facts.latestTask.taskId).toBe(id)
  expect(errors).toEqual([])
  await page.screenshot({path:test.info().outputPath(`page-${locale}-${theme}.png`)})
})

test('native preview policy blocks service workers and leaves opaque page frames free of injected errors',async({page,context})=>{
  const errors=await mount(page,{kind:'application_page',html,contract,contractHash:'fixture'},'en-US','light')
  await expect(page.frameLocator('iframe').frameLocator('iframe').locator('#customer')).toBeVisible()
  expect(await page.locator('iframe').getAttribute('sandbox')).not.toContain('allow-same-origin')
  expect(errors).toEqual([])
  const workerResult=await page.evaluate(()=>new Promise(resolve=>{
    const url=URL.createObjectURL(new Blob(['postMessage(42)'],{type:'text/javascript'}))
    const worker=new Worker(url)
    worker.onmessage=event=>{worker.terminate();URL.revokeObjectURL(url);resolve(event.data)}
  }))
  expect(workerResult).toBe(42)
  const blobRegistration=await page.evaluate(()=>navigator.serviceWorker.register(URL.createObjectURL(new Blob([''],{type:'text/javascript'}))).then(()=> 'registered',error=>error.name))
  expect(blobRegistration).toBe('TypeError')
  const blocked=await page.evaluate(()=>navigator.serviceWorker.register('/forbidden-worker.js').then(()=> 'registered',error=>error.name))
  expect(blocked).toBe('SecurityError')
  expect(context.serviceWorkers()).toHaveLength(0)
  expect(errors).toEqual([expect.stringContaining('worker-src blob:')])
})

for(const locale of ['zh-CN','en-US'])for(const theme of ['light','dark'])test(`actual Workflow renderer displays the frozen node arrangement ${locale} ${theme}`,async({page})=>{
  const state=createEditorStore().getState()
  const nodes:StudioNode[]=[{id:'exit-a',type:'exit',position:{x:380,y:220},data:{editorKind:'exit',key:'result_a',label:'Result A',protected:true,parameters:{outputs:{},errorOutputs:{}}}},{id:'exit-b',type:'exit',position:{x:380,y:440},data:{editorKind:'exit',key:'result_b',label:'Result B',protected:false,parameters:{outputs:{},errorOutputs:{}}}}]
  const document=serializeStudio({...state,nodes,edges:[]})
  const before=JSON.stringify(document)
  const errors=await mount(page,{kind:'workflow',document,manifests:{}},locale,theme)
  await expect(page.getByTestId('exit-node-result_a')).toBeVisible()
  await expect(page.getByTestId('exit-node-result_a')).toBeInViewport()
  await expect(page.getByTestId('exit-node-result_b')).toBeVisible()
  await expect(page.getByTestId('exit-protected')).toBeVisible()
  const facts=await page.evaluate(()=>(window as unknown as PreviewWindow).agentxDesignPreview.facts()) as unknown as {nodeLayouts:unknown}
  expect(facts.nodeLayouts).toEqual(document.editorDocument.nodeLayouts)
  expect(JSON.stringify(document)).toBe(before)
  expect(errors).toEqual([])
  await page.screenshot({path:test.info().outputPath('workflow.png')})
})

test('batch reload preserves the original three rows and never creates another batch',async({page})=>{
  const source=`<button id="submit">Submit three</button><output id="batch"></output><output id="rows"></output><script>
  async function render(id){const b=await agentx.getBatch(id);document.getElementById('batch').textContent=b.batchId;const rows=await agentx.listBatchItems(id,{limit:50});document.getElementById('rows').textContent=rows.items.map(row=>row.clientItemId).join(',')}
  document.getElementById('submit').onclick=async()=>{const b=await agentx.createBatch({name:'Three rows',items:[1,2,3].map(n=>({clientItemId:'row-'+n,input:{customer:'Customer '+n}}))});await render(b.batchId)};
  agentx.getContext().then(c=>{if(c.batchId)render(c.batchId)});
  </script>`
  await mount(page,{kind:'application_page',html:source,contract,contractHash:'fixture'},'zh-CN','light')
  const frame=page.frameLocator('iframe').frameLocator('iframe')
  await frame.locator('#submit').click()
  await expect(frame.locator('#rows')).toHaveText('row-1,row-2,row-3')
  const id=await frame.locator('#batch').innerText()
  await page.evaluate(()=>(window as unknown as PreviewWindow).agentxDesignPreview.reload())
  await page.evaluate(()=>(window as unknown as PreviewWindow).agentxDesignPreview.inspect())
  await expect(frame.locator('#batch')).toHaveText(id)
  await expect(frame.locator('#rows')).toHaveText('row-1,row-2,row-3')
  const facts=await page.evaluate(()=>(window as unknown as PreviewWindow).agentxDesignPreview.facts()) as unknown as {batchCounts:{create:number;items:number}}
  expect(facts.batchCounts).toMatchObject({create:1,items:3})
})

test('compressed image remains readable at the actual authoring viewport',async({page},info)=>{
  const code=`VISION-${crypto.randomUUID().slice(0,8).toUpperCase()}`
  await mount(page,{kind:'application_page',html:`<h1 style="font-size:32px">${code}</h1>`,contract,contractHash:'fixture'},'en-US','light')
  const raw=await page.screenshot({type:'png'})
  const encoded=await page.evaluate(async data=>{
    const image=new Image();image.src=`data:image/png;base64,${data}`;await image.decode()
    const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height
    canvas.getContext('2d')!.drawImage(image,0,0)
    return canvas.toDataURL('image/webp',0.83).split(',')[1]!
  },raw.toString('base64'))
  const image=Buffer.from(encoded,'base64')
  expect(image.length).toBeLessThan(576*1024)
  expect(image.toString('ascii',8,12)).toBe('WEBP')
  writeFileSync(info.outputPath('vision-probe.webp'),image)
  writeFileSync(info.outputPath('vision-probe.json'),JSON.stringify({code,width:1440,height:900,size:image.length}))
})
