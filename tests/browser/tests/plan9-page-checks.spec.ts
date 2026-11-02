import { expect, test } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { inspectPagePreview } from '../../../src/web/src/features/applications/ui/page-checks'
import {hashPageHtml,type PageContract} from '../../../src/web/src/features/applications/ui/page-types'
import { mountPreview } from '../plan9-preview-support'

const base:PageContract={applicationId:'application',applicationName:'Preview fixture',description:null,deploymentId:'deployment',workflowVersionId:'version',bundleId:'bundle',inputSchema:{type:'object',properties:{customer:{type:'string'}},required:['customer']},outputSchema:{type:'object',properties:{answer:{type:'string'}}},approvalStages:[],chatMapping:null,sdkVersion:1,maxUploadBytes:1024,waitInputNodes:[]}
const messages={invoke:'The page must submit through the SDK',updates:'The page must observe task updates',wait:'The page must submit the active input wait'}
const arrayContract:PageContract={...base,inputSchema:{type:'object',properties:{items:{type:'array',minItems:1,maxItems:4,items:{type:'object',properties:{name:{type:'string',minLength:1}},required:['name']}}},required:['items']}}
function arrayHtml(working:boolean,initialRows=2,required=true){return `<form id="form"><div id="items"></div><button type="button" data-action="add-item" data-field-path="/items">Add row</button><button data-action="invoke">Submit</button></form><output></output><script>
const items=document.getElementById('items');
function reindex(){[...items.children].forEach((row,index)=>{row.querySelector('input').dataset.fieldPath='/items/'+index+'/name';row.querySelector('button').dataset.fieldPath='/items/'+index})}
function addRow(){const row=document.createElement('div');row.innerHTML='<label>Name<input ${required?'required':''}></label><button type="button" data-action="remove-item">Remove</button>';row.querySelector('button').onclick=()=>{row.remove();reindex()};items.append(row);reindex()}
for(let index=0;index<${initialRows};index++)addRow();${working?`document.querySelector('[data-action="add-item"]').onclick=()=>addRow();`:''}
function render(task){document.querySelector('output').textContent=JSON.stringify(task.outputs)}
document.querySelector('form').onsubmit=async event=>{event.preventDefault();const task=await agentx.invoke({items:[...items.querySelectorAll('input')].map(field=>({name:field.value}))});render(task);await agentx.subscribeTask(task.taskId,render)};
</script>`}

function historyHtml(working:boolean){return `<form><label>Customer<input data-field-path="/customer" required></label><button data-action="invoke">Submit</button></form><output></output><button type="button" data-action="list-tasks">Refresh history</button><div id="history"></div><script>
function render(task){document.querySelector('output').textContent=JSON.stringify(task.outputs)}
async function load(){const result=await agentx.listMyTasks();document.getElementById('history').innerHTML=result.items.map(task=>'<button type="button" data-task-id="'+task.taskId+'">Open '+task.taskId+'</button>').join('');for(const button of document.querySelectorAll('[data-task-id]'))button.onclick=()=>${working?'agentx.openTask(button.dataset.taskId)':'agentx.getTask(button.dataset.taskId).then(render).catch(error=>document.querySelector(\'output\').textContent=error.message)'}}
document.querySelector('[data-action="list-tasks"]').onclick=load;
document.querySelector('form').onsubmit=async event=>{event.preventDefault();const task=await agentx.invoke({customer:document.querySelector('input').value});render(task);await agentx.subscribeTask(task.taskId,render);await load()};load();
</script>`}

const waitContract:PageContract={...base,waitInputNodes:[{nodeId:'opinion',name:'Opinion',path:'/opinion',responseSchema:{type:'string',minLength:3}}]}
function waitHtml(working:boolean){return `<form id="invoke"><label>Customer<input data-field-path="/customer" required></label><button data-action="invoke">Submit</button></form><output></output><div id="waits"></div><script>
function render(task){document.querySelector('output').textContent=JSON.stringify(task.outputs);const area=document.getElementById('waits');${working?'':`area.innerHTML='';`}
for(const form of area.querySelectorAll('form'))if(!task.pendingInputs.some(wait=>wait.waitId===form.dataset.waitId))form.remove();
for(const wait of task.pendingInputs){if(area.querySelector('[data-wait-id="'+wait.waitId+'"]'))continue;const form=document.createElement('form');form.dataset.waitId=wait.waitId;form.innerHTML='<label>Opinion<textarea data-field-path="" required minlength="3"></textarea></label><button data-action="submit-input">Reply</button>';form.onsubmit=async event=>{event.preventDefault();render(await agentx.submitTaskInput(task.taskId,wait.waitId,form.querySelector('textarea').value))};area.append(form)}}
document.getElementById('invoke').onsubmit=async event=>{event.preventDefault();const task=await agentx.invoke({customer:document.querySelector('input').value});render(task);await agentx.subscribeTask(task.taskId,render)};
</script>`}

type Fixture={contract:PageContract;html:string}
const blankContract:PageContract={...base,inputSchema:{type:'object',properties:{items:{type:'array',minItems:1,maxItems:4,items:{type:'object',properties:{name:{type:'string'}},required:['name']}}},required:['items']}}
const recorded:Partial<Record<'array'|'history'|'wait'|'blank',Fixture>>=process.env.AGENTX_E2E_PREVIEW_FAILURE_FIXTURES?JSON.parse(readFileSync(process.env.AGENTX_E2E_PREVIEW_FAILURE_FIXTURES,'utf8')):{}
for(const [kind,fixture,code] of [
  ['array',{contract:arrayContract,html:arrayHtml(false)},'PAGE_ARRAY_ADD_NOT_WORKING'],
  ['history',{contract:base,html:historyHtml(false)},'PAGE_HISTORY_OPEN_MISSING'],
  ['wait',{contract:waitContract,html:waitHtml(false)},'PAGE_WAIT_FORM_REPLACED'],
  ['blank',{contract:blankContract,html:arrayHtml(true,3,false)},'PAGE_INPUT_ARRAY_LENGTH_CHANGED'],
] as const)test(`Production preview rejects ${kind} interaction defects before publication`,async({page},info)=>{
  const selected=recorded[kind]??fixture
  const {preview,frame}=await mountPreview(page,selected)
  try{
    const report=await inspectPagePreview(frame,preview,messages)
    await info.attach('preview-only-diagnostic',{contentType:'application/json',body:JSON.stringify({report,recordedAiHtml:Boolean(recorded[kind]),formalRequests:preview.counts.formalRequests})})
    expect(report.errors.map(issue=>issue.code)).toContain(code)
    expect(preview.counts.formalRequests).toBe(0)
  }finally{preview.close()}
})

test('A fixed-size array needs no actions that would violate its frozen cardinality',async({page})=>{
  const contract:PageContract={...arrayContract,inputSchema:{type:'object',properties:{items:{type:'array',minItems:2,maxItems:2,items:{type:'object',properties:{name:{type:'string'}},required:['name']}}},required:['items']}}
  const html=`<form><label>First<input required data-field-path="/items/0/name"></label><label>Second<input required data-field-path="/items/1/name"></label><button data-action="invoke">Submit</button></form><output></output><script>
function render(task){document.querySelector('output').textContent=JSON.stringify(task.outputs)}
document.querySelector('form').onsubmit=async event=>{event.preventDefault();const task=await agentx.invoke({items:[...document.querySelectorAll('input')].map(field=>({name:field.value}))});render(task);await agentx.subscribeTask(task.taskId,render)};
</script>`
  const {preview,frame}=await mountPreview(page,{contract,html})
  try{expect((await inspectPagePreview(frame,preview,messages)).errors).toEqual([]);expect(preview.counts.formalRequests).toBe(0)}finally{preview.close()}
})

test('An array already at its maximum is checked by removing and restoring one row',async({page})=>{
  const contract:PageContract={...arrayContract,inputSchema:{type:'object',properties:{items:{type:'array',minItems:1,maxItems:2,items:{type:'object',properties:{name:{type:'string'}},required:['name']}}},required:['items']}}
  const html=`<form><div id="items"></div><button type="button" id="add" data-action="add-item" data-field-path="/items">Add</button><button data-action="invoke">Submit</button></form><output></output><script>
window.rowOperations={added:0,removed:0};const area=document.getElementById('items'),add=document.getElementById('add');
function index(){[...area.children].forEach((row,i)=>{row.querySelector('input').dataset.fieldPath='/items/'+i+'/name';row.querySelector('button').dataset.fieldPath='/items/'+i;row.querySelector('button').disabled=area.children.length<=1});add.disabled=area.children.length>=2}
function append(){if(area.children.length>=2)return;const row=document.createElement('div');row.innerHTML='<label>Name<input required></label><button type="button" data-action="remove-item">Remove</button>';row.querySelector('button').onclick=()=>{if(area.children.length<=1)return;row.remove();window.rowOperations.removed++;index()};area.append(row);window.rowOperations.added++;index()}
append();append();add.onclick=append;
function render(task){document.querySelector('output').textContent=JSON.stringify(task.outputs)}
document.querySelector('form').onsubmit=async event=>{event.preventDefault();const task=await agentx.invoke({items:[...area.querySelectorAll('input')].map(field=>({name:field.value}))});render(task);await agentx.subscribeTask(task.taskId,render)};
</script>`
  const {preview,frame}=await mountPreview(page,{contract,html})
  try{
    expect((await inspectPagePreview(frame,preview,messages)).errors).toEqual([])
    expect(await page.frameLocator('#controller').frameLocator('iframe').locator('body').evaluate(body=>(body.ownerDocument.defaultView as Window&{rowOperations?:unknown}).rowOperations)).toEqual({added:3,removed:1})
    expect(preview.counts.formalRequests).toBe(0)
  }finally{preview.close()}
})

for(const [kind,fixture] of [
  ['array',{contract:arrayContract,html:arrayHtml(true)}],
  ['history',{contract:base,html:historyHtml(true)}],
  ['wait',{contract:waitContract,html:waitHtml(true)}],
] as const)test(`Production preview accepts working ${kind} interactions without business requests`,async({page})=>{
  const {preview,frame}=await mountPreview(page,fixture)
  try{
    const report=await inspectPagePreview(frame,preview,messages)
    expect(report.errors).toEqual([])
    expect(preview.counts.formalRequests).toBe(0)
  }finally{preview.close()}
})


for(const formOnly of [false,true])test(`Production preview submits native marked forms${formOnly?' without a marked button':' with a correlated marked button'}`,async({page})=>{
  let html=waitHtml(true).replace('<form id="invoke">','<form id="invoke" data-action="invoke">').replace('form.dataset.waitId=wait.waitId;','form.dataset.waitId=wait.waitId;form.dataset.action="submit-input";')
  if(formOnly)html=html.replace('<button data-action="invoke">','<button>').replace('<button data-action="submit-input">','<button>')
  const {preview,frame}=await mountPreview(page,{contract:waitContract,html})
  try{
    expect((await inspectPagePreview(frame,preview,messages)).errors).toEqual([])
    expect(preview.counts).toMatchObject({invoke:1,input:1,formalRequests:0})
    expect(preview.latest()?.state).toBe('completed')
  }finally{preview.close()}
})

test('A marked native form retains browser constraints instead of bypassing validation',async({page})=>{
  const html=`<form data-action="invoke"><label>Customer<input required data-field-path="/customer"></label><label>Unconfigured required value<input required data-field-path="/unconfigured"></label><button>Submit</button></form><script>
  document.querySelector('form').onsubmit=event=>{event.preventDefault();agentx.invoke({customer:document.querySelector('input').value})};
  </script>`
  const {preview,frame}=await mountPreview(page,{contract:base,html})
  try{
    const report=await inspectPagePreview(frame,preview,messages)
    expect(report.errors.map(issue=>issue.code)).toContain('PAGE_FIELD_CONSTRAINT_MISMATCH')
    expect(preview.counts).toMatchObject({invoke:0,formalRequests:0})
    expect(preview.latest()).toBeUndefined()
  }finally{preview.close()}
})


const correctedSource: (Fixture&{provenance:{htmlHash:string;rawSha256:string;runIds:string[]}})|undefined=process.env.AGENTX_E2E_PREVIEW_CORRECTED_FIXTURE?JSON.parse(readFileSync(process.env.AGENTX_E2E_PREVIEW_CORRECTED_FIXTURE,'utf8')):undefined
if(correctedSource)test('The exact previously rejected AI HTML passes the corrected native action inspector',async({page},info)=>{
  expect(await hashPageHtml(correctedSource.html)).toBe(correctedSource.provenance.htmlHash)
  expect(correctedSource.provenance.runIds).toHaveLength(3)
  const {preview,frame}=await mountPreview(page,correctedSource)
  try{
    const report=await inspectPagePreview(frame,preview,messages)
    expect(report.errors).toEqual([])
    expect(preview.counts.invoke).toBe(1)
    expect(preview.counts.input).toBeGreaterThan(0)
    expect(preview.latest()?.state).toBe('completed')
    expect(preview.counts.formalRequests).toBe(0)
    await info.attach('original-html-fixed-inspector',{contentType:'application/json',body:JSON.stringify({provenance:correctedSource.provenance,report,counts:preview.counts,unmodifiedOriginalHtml:true,newAuthoringRuns:0,formalRequests:0})})
  }finally{preview.close()}
})
