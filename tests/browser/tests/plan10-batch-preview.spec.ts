import {expect,test} from '@playwright/test'
import {mountPreview} from '../plan9-preview-support'
import {inspectPagePreview} from '../../../src/web/src/features/applications/ui/page-checks'
import type {PageContract} from '../../../src/web/src/features/applications/ui/page-types'

const contract:PageContract={applicationId:'app',applicationName:'Batch preview',deploymentId:'deployment',workflowVersionId:'version',bundleId:'bundle',inputSchema:{type:'object',properties:{customer:{type:'string'}},required:['customer']},outputSchema:{type:'object',properties:{answer:{type:'string'}}},waitInputNodes:[],approvalStages:[],chatMapping:null,sdkVersion:2,maxUploadBytes:1048576,description:null}
const messages={invoke:'Submit one task',updates:'Observe task updates',wait:'Complete original input waits'}

function html(kind:'working'|'no-poll'|'loop'|'duplicate'){
  return `<button type="button" data-action="batch-mode" id="mode">Batch</button>
  <form id="single"><label>Customer<input data-field-path="/customer" name="customer" required></label><button data-action="invoke">Single</button></form><p id="task-status"></p>
  <section id="batch" hidden><div id="rows"></div><button type="button" data-action="add-batch-item" id="add">Add row</button><button type="button" data-action="create-batch" id="submit">Submit batch</button><p id="batch-status"></p><button type="button" data-action="list-batches" id="history">My batches</button><div id="history-rows"></div></section>
  <script>
  const single=document.getElementById('single'),area=document.getElementById('batch'),rows=document.getElementById('rows');let timer;
  const renderTask=task=>document.getElementById('task-status').textContent=task.state;
  single.onsubmit=async e=>{e.preventDefault();const task=await agentx.invoke({customer:single.elements.customer.value});renderTask(task);await agentx.subscribeTask(task.taskId,renderTask)};
  document.getElementById('mode').onclick=()=>{single.hidden=true;area.hidden=false};
  function add(){const row=document.createElement('div');row.dataset.batchRow='';row.dataset.clientItemId=${kind==='duplicate'?"'same'":'crypto.randomUUID()'};row.innerHTML='<label>Customer<input data-field-path="/customer" required></label><button type="button" data-action="remove-batch-item">Remove</button>';row.querySelector('button').onclick=()=>row.remove();rows.append(row)}
  document.getElementById('add').onclick=add;add();
  document.getElementById('submit').onclick=async()=>{const items=[...rows.children].map(row=>({clientItemId:row.dataset.clientItemId,input:{customer:row.querySelector('input').value}}));
    ${kind==='loop'?"for(const item of items)await agentx.invoke(item.input);return;":''}
    const batch=await agentx.createBatch({name:'Preview batch',items});document.getElementById('batch-status').textContent=batch.state;
    ${kind==='no-poll'?'':"clearInterval(timer);timer=setInterval(async()=>{const current=await agentx.getBatch(batch.batchId);await agentx.listBatchItems(batch.batchId,{limit:50});document.getElementById('batch-status').textContent=current.state;if(current.counts.ended===current.counts.total)clearInterval(timer)},100);"}
  };
  document.getElementById('history').onclick=async()=>{const page=await agentx.listMyBatches();const list=document.getElementById('history-rows');list.replaceChildren();for(const batch of page.items){const entry=document.createElement('button');entry.type='button';entry.dataset.batchId=batch.batchId;entry.textContent=batch.name;entry.onclick=()=>agentx.openBatch(batch.batchId);list.append(entry)}};
  </script>`
}

for(const kind of ['working','no-poll','loop','duplicate'] as const)test(`Batch inspector checks ${kind} pages through real opaque SDK`,async({page})=>{
  const{preview,frame}=await mountPreview(page,{contract,html:html(kind)})
  try{
    const report=await inspectPagePreview(frame,preview,messages)
    if(kind==='working'){
      expect(report.errors).toEqual([])
      expect(preview.batches.counts).toMatchObject({create:1,items:3,historyOpened:1})
      expect(preview.counts.formalRequests).toBe(0)
    }else if(kind==='no-poll')expect(report.errors.map(e=>e.code)).toContain('PAGE_BATCH_UPDATES_MISSING')
    else if(kind==='loop')expect(report.errors.map(e=>e.code)).toContain('PAGE_BATCH_SUBMISSION_COUNT')
    else expect(report.errors.map(e=>e.code)).toContain('PAGE_BATCH_ROW_ID')
  }finally{preview.close()}
})

test('script diagnostics point to the original HTML instead of injected SDK lines',async({page})=>{
  const source='<h1>Broken script</h1>\n<script>\nconst broken = ;\n</script>'
  const {preview,frame}=await mountPreview(page,{contract,html:source,stylesheet:'/* Platform styles */\r\nbody { color: black }\r\n'})
  try{
    const report=await frame.inspect()
    const error=report.errors.find(issue=>issue.code==='PAGE_SCRIPT_ERROR')
    expect(error).toMatchObject({line:3})
    expect(error?.column).toBeGreaterThan(0)
    expect(preview.counts.formalRequests).toBe(0)
  }finally{preview.close()}
})
