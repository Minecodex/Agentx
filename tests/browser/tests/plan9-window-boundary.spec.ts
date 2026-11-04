import {expect,test} from '@playwright/test'
import {readFileSync} from 'node:fs'
import {createPageController} from '../../../src/web/src/features/applications/ui/page-isolation'

const sdk=readFileSync(new URL('../../../contracts/sdk/application-page-v2.js',import.meta.url),'utf8')
const nginx=readFileSync(new URL('../../../deploy/docker/nginx-v2.conf',import.meta.url),'utf8')
// Exercise the declared response policies in a real browser. The deployed SPA
// response headers are also asserted by the final catalog/browser regression.
const headers=Object.fromEntries([...nginx.matchAll(/add_header\s+(Cross-Origin-Opener-Policy|X-Frame-Options)\s+"?([\w-]+)"?\s+always;/g)].map(match=>[match[1]!,match[2]!]))
const host='https://host.agentx.test/'
const attacker='https://untrusted.agentx.test/'
const binding='plan9_foreign_window_fixture'
const html=`<h1>Private business page</h1><script>addEventListener('message',event=>{if(event.data?.kind==='read-business')agentx.listMyTasks().then(value=>event.source.postMessage({type:'page-leak',value},'*'))});</script>`
const controller=createPageController(html,binding,sdk.replace('__AGENTX_PAGE_BINDING__',JSON.stringify(binding)))
const source=`<iframe id="controller" sandbox="allow-scripts allow-forms"></iframe><script>
  const frame=document.querySelector('iframe');addEventListener('message',event=>{
    if(event.source!==frame.contentWindow||event.data?.type!=='agentx.ui.ready'||event.data.bindingId!==${JSON.stringify(binding)})return;
    const channel=new MessageChannel();channel.port1.onmessage=event=>{if(event.data.type==='request')channel.port1.postMessage({type:'response',id:event.data.id,result:{items:[{outputs:{marker:'synthetic-private-result'}}],next:null}})};
    frame.contentWindow.postMessage({type:'agentx.ui.bind',bindingId:${JSON.stringify(binding)}},'*',[channel.port2]);
  });frame.srcdoc=${JSON.stringify(controller).replaceAll('<','\\u003c')};
</script>`

test('A foreign opener cannot relay the finite SDK through an untrusted generated page',async({page,context},info)=>{
  await context.route(host,route=>route.fulfill({status:200,contentType:'text/html',headers,body:source}))
  await context.route(attacker,route=>route.fulfill({status:200,contentType:'text/html',body:'<script>window.leaks=[];addEventListener("message",event=>{if(event.data?.type==="page-leak")window.leaks.push(event.data.value)})</script>'}))
  await page.goto(attacker)
  const opened=page.waitForEvent('popup')
  await page.evaluate(url=>{(window as Window&{victim?:Window|null}).victim=window.open(url)},host)
  const popup=await opened
  await expect(popup.frameLocator('#controller').frameLocator('iframe').getByRole('heading',{name:'Private business page'})).toBeVisible()
  for(let index=0;index<12;index++){
    await page.evaluate(()=>{
      try{(window as Window&{victim?:Window}).victim!.frames[0]!.frames[0]!.postMessage({kind:'read-business'},'*')}catch{/* A severed browsing context must reject the reference. */}
    })
    await page.waitForTimeout(50)
  }
  const leaks=await page.evaluate(()=>(window as Window&{leaks?:unknown[]}).leaks)
  await info.attach('foreign-window-result',{contentType:'application/json',body:JSON.stringify({headers,leaks})})
  expect(leaks).toEqual([])
  expect(await page.evaluate(()=>(window as Window&{victim?:Window}).victim!.closed)).toBe(true)
  await popup.close()
})

test('A foreign page cannot embed the trusted application shell',async({page,context})=>{
  await context.route(host,route=>route.fulfill({status:200,contentType:'text/html',headers,body:'<h1>Trusted application shell</h1>'}))
  await context.route(attacker,route=>route.fulfill({status:200,contentType:'text/html',body:`<iframe src="${host}" title="foreign embed"></iframe>`}))
  await page.goto(attacker)
  await page.waitForTimeout(300)
  await expect(page.frameLocator('iframe').getByRole('heading',{name:'Trusted application shell'})).not.toBeVisible()
})
