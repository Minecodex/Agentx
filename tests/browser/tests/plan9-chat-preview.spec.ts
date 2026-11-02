import {expect,test} from '@playwright/test'
import {readFileSync} from 'node:fs'
import {inspectPagePreview} from '../../../src/web/src/features/applications/ui/page-checks'
import {hashPageHtml,type PageContract} from '../../../src/web/src/features/applications/ui/page-types'
import {mountPreview} from '../plan9-preview-support'

const base:PageContract={applicationId:'application',applicationName:'Conversation',description:null,deploymentId:'deployment',workflowVersionId:'version',bundleId:'bundle',inputSchema:{type:'object',properties:{message:{type:'string'}},required:['message']},outputSchema:{type:'object',properties:{answer:{type:'string'}}},approvalStages:[],chatMapping:null,sdkVersion:1,maxUploadBytes:1024,waitInputNodes:[]}
const chat:PageContract={...base,chatMapping:{questionInput:'message',fileInput:null,answerOutput:'answer',answerFilesOutput:null}}
const messages={invoke:'Submit through the SDK',updates:'Observe the original task',wait:'Submit the original active input wait'}
function chatHtml(restore:boolean,formal=true,marked=true){return `<form><label>Message<input data-field-path="/message" required></label><button data-action="${marked?'send-message':'invoke'}">Send</button></form><output></output><script>
let sessionId=null;function render(task){document.querySelector('output').textContent=JSON.stringify(task.outputs)}
const ready=agentx.getContext().then(async context=>{if(context.taskId){const task=await agentx.getTask(context.taskId);${restore?'sessionId=task.sessionId;await agentx.getSession(sessionId);':''}render(task);await agentx.subscribeTask(task.taskId,render)}});
document.querySelector('form').onsubmit=async event=>{event.preventDefault();await ready;${formal?'if(!sessionId)sessionId=(await agentx.createSession({})).id;const task=await agentx.sendMessage(sessionId,document.querySelector(\'input\').value);':'const task=await agentx.invoke({message:document.querySelector(\'input\').value});'}render(task);await agentx.subscribeTask(task.taskId,render)};
</script>`}
const wait:PageContract={...base,waitInputNodes:[{nodeId:'details',name:'Details',path:'/details',responseSchema:{type:'string',minLength:3}}]}
function waitHtml(show:boolean){return `<form id="initial"><label>Message<input data-field-path="/message" required></label><button data-action="invoke">Submit</button></form><output></output><section id="area" hidden><div id="waits" hidden></div></section><script>
function render(task){document.querySelector('output').textContent=JSON.stringify(task.outputs);const area=document.getElementById('area'),waits=document.getElementById('waits');area.hidden=!task.pendingInputs.length;${show?'waits.hidden=!task.pendingInputs.length;':''}for(const form of waits.querySelectorAll('form'))if(!task.pendingInputs.some(wait=>wait.waitId===form.dataset.waitId))form.remove();for(const wait of task.pendingInputs){if(waits.querySelector('[data-wait-id="'+wait.waitId+'"]'))continue;const form=document.createElement('form');form.dataset.waitId=wait.waitId;form.innerHTML='<label>Details<textarea data-field-path="" required minlength="3"></textarea></label><button data-action="submit-input">Reply</button>';form.onsubmit=async event=>{event.preventDefault();render(await agentx.submitTaskInput(task.taskId,wait.waitId,form.querySelector('textarea').value))};waits.append(form)}}
document.getElementById('initial').onsubmit=async event=>{event.preventDefault();const task=await agentx.invoke({message:document.querySelector('input').value});render(task);await agentx.subscribeTask(task.taskId,render)};
</script>`}

type Fixture={contract:PageContract;html:string;provenance?:{htmlHash:string;source:string}}
const recorded:Record<string,Fixture>=process.env.AGENTX_E2E_PREVIEW_V42_FAILURE_FIXTURES?JSON.parse(readFileSync(process.env.AGENTX_E2E_PREVIEW_V42_FAILURE_FIXTURES,'utf8')):{}
const failures:Array<[string,Fixture,string]>=[
  ['chat_restore',{contract:chat,html:chatHtml(false)},'PAGE_CHAT_SESSION_NOT_RESTORED'],
  ['chat_methods',{contract:chat,html:chatHtml(true,false)},'PAGE_CHAT_METHODS_REQUIRED'],
  ['chat_action',{contract:chat,html:chatHtml(true,true,false)},'PAGE_SUBMIT_ACTION_MISSING'],
  ['hidden_wait',{contract:wait,html:waitHtml(false)},'PAGE_FIELD_HIDDEN'],
]
if(recorded.chat_restore_repair)failures.push(['chat_restore_repair',recorded.chat_restore_repair,'PAGE_CHAT_SESSION_NOT_RESTORED'])
for(const [name,synthetic,code]of failures)test(`Production preview rejects ${name} before publication`,async({page},info)=>{
  const fixture=recorded[name]??synthetic
  if(recorded[name])expect(await hashPageHtml(fixture.html)).toBe(recorded[name]!.provenance!.htmlHash)
  const {preview,frame}=await mountPreview(page,fixture)
  try{
    const report=await inspectPagePreview(frame,preview,messages)
    await info.attach('preview-only-diagnostic',{contentType:'application/json',body:JSON.stringify({report,recordedAiHtml:Boolean(recorded[name]),provenance:fixture.provenance,formalRequests:preview.counts.formalRequests})})
    expect(report.errors.map(issue=>issue.code)).toContain(code)
    expect(preview.counts.formalRequests).toBe(0)
  }finally{preview.close()}
})

test('A formal chat survives a recreated opaque page and continues its original session',async({page})=>{
  const {preview,frame}=await mountPreview(page,{contract:chat,html:chatHtml(true)})
  try{
    expect((await inspectPagePreview(frame,preview,messages)).errors).toEqual([])
    expect(preview.chat).toMatchObject({created:1,sent:2,directInvocations:0})
    expect(preview.counts).toMatchObject({invoke:2,formalRequests:0})
  }finally{preview.close()}
})

test('An active input wait becomes visible without losing its typed values',async({page})=>{
  const {preview,frame}=await mountPreview(page,{contract:wait,html:waitHtml(true)})
  try{expect((await inspectPagePreview(frame,preview,messages)).errors).toEqual([]);expect(preview.counts).toMatchObject({input:1,formalRequests:0})}finally{preview.close()}
})

test('An accessible upload label can use a hidden native file control',async({page})=>{
  const contract:PageContract={...base,inputSchema:{type:'object',properties:{message:{type:'string'},attachment:{type:'object',properties:{artifactId:{type:'string',format:'uuid'},contentType:{type:'string'},sizeBytes:{type:'integer'},sha256:{type:'string'}},required:['artifactId','contentType','sizeBytes','sha256']}},required:['message','attachment']}}
  const html=`<form><label>Message<input data-field-path="/message"></label><label for="file">Upload attachment</label><input type="file" id="file" data-field-path="/attachment" style="display:none"><button data-action="invoke">Submit</button></form><output></output><script>
function render(task){document.querySelector('output').textContent=JSON.stringify(task.outputs)}
document.querySelector('form').onsubmit=async event=>{event.preventDefault();const attachment=await agentx.uploadFile(document.getElementById('file').files[0]);const task=await agentx.invoke({message:document.querySelector('[data-field-path="/message"]').value,attachment});render(task);await agentx.subscribeTask(task.taskId,render)};
</script>`
  const {preview,frame}=await mountPreview(page,{contract,html})
  try{expect((await inspectPagePreview(frame,preview,messages)).errors).toEqual([]);expect(preview.counts).toMatchObject({invoke:1,upload:1,formalRequests:0})}finally{preview.close()}
})

test('Loading a page cannot submit an extra synthetic business task before the marked action',async({page})=>{
  const html=`<form><label>Message<input data-field-path="/message"></label><button data-action="invoke">Submit</button></form><output></output><script>
function render(task){document.querySelector('output').textContent=JSON.stringify(task.outputs)}
agentx.invoke({message:'Unrequested synthetic task'}).then(task=>agentx.subscribeTask(task.taskId,render));
document.querySelector('form').onsubmit=async event=>{event.preventDefault();const task=await agentx.invoke({message:document.querySelector('input').value});render(task);await agentx.subscribeTask(task.taskId,render)};
</script>`
  const {preview,frame}=await mountPreview(page,{contract:base,html})
  try{expect((await inspectPagePreview(frame,preview,messages)).errors.map(issue=>issue.code)).toContain('PAGE_PREVIEW_SUBMISSION_COUNT');expect(preview.counts.formalRequests).toBe(0)}finally{preview.close()}
})
