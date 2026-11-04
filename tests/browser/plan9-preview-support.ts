import { expect, type Page } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { createPageController } from '../../src/web/src/features/applications/ui/page-isolation'
import { pageSourceIssues } from '../../src/web/src/features/applications/ui/page-diagnostics'
import { createPagePreview } from '../../src/web/src/features/applications/ui/page-preview'
import { hashPageHtml, type PageContract, type PageInspection, type PageInspectionOptions, type PageFrameHandle } from '../../src/web/src/features/applications/ui/page-types'

type Preview = ReturnType<typeof createPagePreview>
export type PreviewHost = Window & {
  privatePort?: MessagePort; ready?: boolean; reports?: Record<string, (value:PageInspection)=>void>
  inspectPage?: (input:unknown, waitId?:string, submit?:boolean, options?:PageInspectionOptions)=>Promise<PageInspection>
  previewRequest?: (value:{method:Parameters<Preview['dispatch']>[0];args:Record<string,unknown>;id:string})=>Promise<unknown>
}

/** Actual opaque page SDK and the production synthetic dispatcher, with no business endpoint. */
export async function mountPreview(page:Page, selected:{contract:PageContract;html:string;stylesheet?:string}) {
  const sdk = readFileSync(new URL('../../contracts/sdk/application-page-v2.js', import.meta.url), 'utf8')
  const preview = createPagePreview(selected.contract, 'fixture-hash', 'zh-CN', 'light')
  await page.exposeBinding('previewRequest', async(_source,value:{method:Parameters<Preview['dispatch']>[0];args:Record<string,unknown>;id:string})=>{
    // Playwright bindings serialize JSON; MessagePort itself transfers buffers.
    // Restore the actual synthetic bytes after crossing this test-only boundary.
    if(value.method==='uploadFile'&&Array.isArray(value.args.bytes))value.args.bytes=new Uint8Array(value.args.bytes).buffer
    return preview.dispatch(value.method,value.args,value.id,(subscriptionId,task)=>{
      void page.evaluate(({subscriptionId,task})=>(window as PreviewHost).privatePort?.postMessage({type:'task',subscriptionId,task}),{subscriptionId,task}).catch(()=>undefined)
    })
  })
  await page.route('https://preview.agentx.test/', route=>route.fulfill({contentType:'text/html',body:'<iframe id="controller" sandbox="allow-scripts allow-forms"></iframe>'}))
  const restartFrame = async()=>{
    await page.evaluate(()=>(window as PreviewHost).privatePort?.close())
    const binding=crypto.randomUUID().replaceAll('-','_')
    const source=createPageController(selected.html,binding,sdk.replace('__AGENTX_PAGE_BINDING__',JSON.stringify(binding)),selected.stylesheet)
    await page.goto('https://preview.agentx.test/')
    await page.evaluate(({source,binding})=>{
      const host=window as PreviewHost,frame=document.querySelector<HTMLIFrameElement>('#controller')!
      host.reports={}
      addEventListener('message',event=>{
        if(event.source!==frame.contentWindow||event.data?.type!=='agentx.ui.ready'||event.data.bindingId!==binding)return
        const channel=new MessageChannel();host.privatePort=channel.port1
        channel.port1.onmessage=event=>{
          const message=event.data
          if(message.type==='request'){
            if(message.method==='uploadFile'&&message.args.bytes instanceof ArrayBuffer)message.args.bytes=Array.from(new Uint8Array(message.args.bytes))
            void host.previewRequest!(message).then(result=>channel.port1.postMessage({type:'response',id:message.id,result}),error=>channel.port1.postMessage({type:'response',id:message.id,error:{code:error?.code,message:String(error)}}))
          }
          if(message.type==='inspection')host.reports![message.id]?.(message.report)
        }
        host.inspectPage=(input,waitId,submit=true,options)=>new Promise((resolve,reject)=>{
          const id=crypto.randomUUID(),timer=setTimeout(()=>{delete host.reports![id];reject(new Error('PAGE_INSPECTION_TIMEOUT'))},15_000)
          host.reports![id]=value=>{clearTimeout(timer);delete host.reports![id];resolve(value)}
          channel.port1.postMessage({type:'inspect',id,input,waitId,submit,options})
        })
        frame.contentWindow!.postMessage({type:'agentx.ui.bind',bindingId:binding},'*',[channel.port2]);host.ready=true
      });frame.srcdoc=source
    },{source,binding})
    await expect.poll(()=>page.evaluate(()=>(window as PreviewHost).ready)).toBe(true)
  }
  const frame:PageFrameHandle={
    restart:restartFrame,
    inspect:async(input,submit=false,waitId,options)=>{
      const report=await page.evaluate(({input,submit,waitId,options})=>(window as PreviewHost).inspectPage!(input,waitId,submit,options),{input,submit,waitId,options})
      return {...report,errors:pageSourceIssues(report.errors,sdk,selected.stylesheet),sourceHash:await hashPageHtml(selected.html)}
    },
  }
  const restart=async()=>{preview.reset();await frame.restart()}
  await restart()
  return {preview,frame,restart}
}
