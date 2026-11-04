import {expect,test,type Page} from '@playwright/test'
import {readFileSync} from 'node:fs'
import {createPageController} from '../../../src/web/src/features/applications/ui/page-isolation'
import {createPagePreview,sampleValue} from '../../../src/web/src/features/applications/ui/page-preview'
import type {PageContract} from '../../../src/web/src/features/applications/ui/page-types'
import {fillFields,waitInputScope,waitInputAction} from '../plan9-page-support'

const field='<label>Contact<input data-field-path="/contact"></label>'
const action='<button data-wait-id="delivery" data-action="submit-input">Send</button>'
for(const [name,markup] of [
  ['form and button',`<form data-wait-id="delivery">${field}${action}</form>`],
  ['native marked form',`<form data-wait-id="delivery" data-action="submit-input">${field}<button>Send</button></form>`],
  ['card and form',`<div data-wait-id="delivery"><form data-wait-id="delivery">${field}${action}</form></div>`],
  ['card and div',`<section data-wait-id="delivery"><div data-wait-id="delivery">${field}${action}</div></section>`],
  ['form and fieldset',`<form data-wait-id="delivery"><fieldset data-wait-id="delivery">${field}${action}</fieldset></form>`],
  ['action inside button',`<div data-wait-id="delivery"><div data-wait-id="delivery">${field}<button data-wait-id="delivery"><span data-action="submit-input">Send</span></button></div></div>`],
])test(`Correlated ${name} selects one wait container; independent forms remain ambiguous`,async({page})=>{
  await page.setContent(markup!)
  await page.evaluate(()=>{document.querySelector('button')!.onclick=event=>{event.preventDefault();document.body.dataset.submitted=document.querySelector('input')!.value}})
  const scope=waitInputScope(page.mainFrame(),'delivery')
  await expect(scope).toHaveCount(1)
  await fillFields(scope,{contact:'Preview contact'})
  await(await waitInputAction(scope)).click()
  await expect(page.locator('body')).toHaveAttribute('data-submitted','Preview contact')
  await page.evaluate(()=>document.body.append(document.body.firstElementChild!.cloneNode(true)))
  await expect(waitInputScope(page.mainFrame(),'delivery')).toHaveCount(2)
})

const contract:PageContract={applicationId:'application',applicationName:'Delivery',description:null,deploymentId:'deployment',workflowVersionId:'version',bundleId:'bundle',inputSchema:{type:'object',properties:{request_id:{type:'string'}}},outputSchema:{type:'object',properties:{result:{type:'string'}}},approvalStages:[],chatMapping:null,sdkVersion:1,maxUploadBytes:1024,waitInputNodes:[{nodeId:'delivery',name:'Delivery',path:'delivery',responseSchema:{type:'object',properties:{date:{type:'string',format:'date'},contact:{type:'string'},email:{type:'string',format:'email'}},required:['date','contact','email']}}]}
const html=`<label>Request<input data-field-path="/request_id" id="request"></label><button data-action="invoke" id="invoke">Submit</button><div id="waits"></div><script>
function render(task){document.getElementById('waits').innerHTML=task.pendingInputs.map(wait=>'<form data-wait-id="'+wait.waitId+'"><label>Date<input type="date" required data-field-path="/date"></label><label>Contact<input required data-field-path="/contact"></label><label>Email<input type="email" required data-field-path="/email"></label><button data-action="submit-input">Send</button></form>').join('');for(const form of document.querySelectorAll('form'))form.onsubmit=event=>{event.preventDefault();const content={};for(const field of form.querySelectorAll('input'))content[field.dataset.fieldPath.slice(1)]=field.value;agentx.submitTaskInput(task.taskId,form.dataset.waitId,content).then(render)}};
document.getElementById('invoke').onclick=async()=>{const task=await agentx.invoke({request_id:document.getElementById('request').value});render(task);await agentx.subscribeTask(task.taskId,render)};
</script>`
const fixture: {contract:PageContract;html:string}=process.env.AGENTX_E2E_PREVIEW_FIXTURE?JSON.parse(readFileSync(process.env.AGENTX_E2E_PREVIEW_FIXTURE,'utf8')):{contract,html}

type Preview=ReturnType<typeof createPagePreview>
type Host=Window&{privatePort?:MessagePort;ready?:boolean;reports?:Record<string,(value:unknown)=>void>;inspectPage?:(input:unknown,waitId?:string)=>Promise<{errors:unknown[]}>;previewRequest?:(value:{method:Parameters<Preview['dispatch']>[0];args:Record<string,unknown>;id:string})=>Promise<unknown>}
async function mountPreview(page:Page,selected:{contract:PageContract;html:string}){
  const sdk=readFileSync(new URL('../../../contracts/sdk/application-page-v2.js',import.meta.url),'utf8')
  const preview=createPagePreview(selected.contract,'fixture-hash','zh-CN','light')
  await page.exposeBinding('previewRequest',async(_source,value:{method:Parameters<typeof preview.dispatch>[0];args:Record<string,unknown>;id:string})=>preview.dispatch(value.method,value.args,value.id,(subscriptionId,task)=>{void page.evaluate(({subscriptionId,task})=>(window as Host).privatePort?.postMessage({type:'task',subscriptionId,task}),{subscriptionId,task}).catch(()=>undefined)}))
  await page.route('https://preview.agentx.test/',route=>route.fulfill({contentType:'text/html',body:'<iframe id="controller" sandbox="allow-scripts allow-forms"></iframe>'}))
  const restart=async()=>{
  await page.evaluate(()=>(window as Host).privatePort?.close())
  preview.reset()
  const binding=crypto.randomUUID().replaceAll('-','_')
  const source=createPageController(selected.html,binding,sdk.replace('__AGENTX_PAGE_BINDING__',JSON.stringify(binding)))
  await page.goto('https://preview.agentx.test/')
  await page.evaluate(({source,binding})=>{
    const host=window as Host,frame=document.querySelector<HTMLIFrameElement>('#controller')!
    host.reports={}
    addEventListener('message',event=>{
      if(event.source!==frame.contentWindow||event.data?.type!=='agentx.ui.ready'||event.data.bindingId!==binding)return
      const channel=new MessageChannel();host.privatePort=channel.port1
      channel.port1.onmessage=event=>{
        const message=event.data
        if(message.type==='request')void host.previewRequest!(message).then(result=>channel.port1.postMessage({type:'response',id:message.id,result}),error=>channel.port1.postMessage({type:'response',id:message.id,error:{message:String(error)}}))
        if(message.type==='inspection')host.reports![message.id]?.(message.report)
      }
      host.inspectPage=(input,waitId)=>new Promise(resolve=>{const id=crypto.randomUUID();host.reports![id]=value=>{delete host.reports![id];resolve(value as {errors:unknown[]})};channel.port1.postMessage({type:'inspect',id,input,waitId,submit:true})})
      frame.contentWindow!.postMessage({type:'agentx.ui.bind',bindingId:binding},'*',[channel.port2]);host.ready=true
    });frame.srcdoc=source
  },{source,binding})
  await expect.poll(()=>page.evaluate(()=>(window as Host).ready)).toBe(true)
  }
  await restart()
  return {preview,restart}
}

test('Repeated isolated checks allow a structured reply without inventing task changes',async({page},info)=>{
  const {preview,restart}=await mountPreview(page,fixture)
  try{
    await expect.poll(()=>page.evaluate(()=>(window as Host).ready)).toBe(true)
    const taskIds=new Set<string>()
    for(let round=0;round<2;round++){
      await restart()
      expect(await preview.dispatch('listMyTasks',{},'initial-history',()=>undefined)).toEqual({items:[],next:null})
      const before=preview.counts.invoke
      const initial=await page.evaluate(input=>(window as Host).inspectPage!(input),preview.input)
      expect(initial.errors).toEqual([])
      await expect.poll(()=>preview.counts.invoke).toBe(before+1)
      await expect.poll(()=>preview.latest()?.state).toBe('waiting_input')
      await page.waitForTimeout(300)
      const current=preview.latest()!,wait=current.pendingInputs[0]!,inputs=preview.counts.input
      taskIds.add(current.taskId)
      const checked=await page.evaluate(({input,waitId})=>(window as Host).inspectPage!(input,waitId),{input:sampleValue(wait.responseSchema),waitId:wait.waitId})
      expect(checked.errors).toEqual([])
      await expect.poll(()=>preview.counts.input).toBe(inputs+1)
      expect(preview.latest()).toMatchObject({taskId:current.taskId,state:'completed'})
    }
    expect(taskIds.size).toBe(2)
    expect(preview.counts.formalRequests).toBe(0)
    await info.attach('preview-only-proof',{contentType:'application/json',body:JSON.stringify({counts:preview.counts,usesRecordedAiHtml:Boolean(process.env.AGENTX_E2E_PREVIEW_FIXTURE),realBusinessEvidence:false})})
  }finally{preview.close()}
})

test('Preview reports malformed pointer-key submissions through the isolated SDK',async({page})=>{
  const {preview}=await mountPreview(page,{contract,html:html.replace('field.dataset.fieldPath.slice(1)','field.dataset.fieldPath')})
  try{
    await page.evaluate(input=>(window as Host).inspectPage!(input),preview.input)
    await expect.poll(()=>preview.latest()?.state).toBe('waiting_input')
    await page.waitForTimeout(300)
    const task=preview.latest()!,wait=task.pendingInputs[0]!
    const checked=await page.evaluate(({input,waitId})=>(window as Host).inspectPage!(input,waitId),{input:sampleValue(wait.responseSchema),waitId:wait.waitId})
    expect(JSON.stringify(checked.errors)).toContain('submitTaskInput.content')
    expect(preview.latest()).toMatchObject({taskId:task.taskId,state:'waiting_input',pendingInputs:[{waitId:wait.waitId}]})
    expect(preview.counts).toMatchObject({input:0,formalRequests:0})
  }finally{preview.close()}
})
