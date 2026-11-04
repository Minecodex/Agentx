import type { PageMethod } from './page-bridge'
import type { PageContract, TaskView } from './page-types'
import type { BatchView, BatchItems, BatchExport } from './page-batch-live'

type Row={itemId:string;clientItemId:string;input:unknown;taskId?:string;cancelled?:boolean}
type Synthetic={view:BatchView;rows:Row[]}
const methods=new Set<PageMethod>(['createBatch','listMyBatches','getBatch','listBatchItems','openBatch','stopBatch','cancelBatch','rerunFailedItems','exportBatch','getBatchExport','downloadBatchExport','readTable'])
const deny=(message:string)=>Object.assign(new Error(message),{code:'PREVIEW_BATCH_INVALID'})

export function createBatchPreview(contract:PageContract,invoke:(input:unknown)=>TaskView,load:(id:string)=>TaskView,observe:(task:TaskView)=>void,sample:(schema:unknown)=>unknown){
  const batches=new Map<string,Synthetic>(),exports=new Map<string,BatchExport>()
  const counts={create:0,items:0,observations:0,observedRevision:0,historyListed:0,historyOpened:0,lastHistoryOpened:''}
  let latestId:string|undefined,historyId:string|undefined,submitted:unknown[]=[]
  const owned=(id:string,history=false)=>{const value=batches.get(id);if(!value)throw deny('Synthetic batch was not found');if(id===historyId&&!history)throw deny('Open the previous publication through openBatch');return value}
  const advance=(batch:Synthetic)=>{
    if(batch.view.dispatchState!=='active')return
    const active=batch.rows.filter(r=>r.taskId&&['accepted','running'].includes(load(r.taskId).state)).length
    for(const row of batch.rows.filter(r=>!r.taskId&&!r.cancelled).slice(0,Math.max(0,4-active)))row.taskId=invoke(row.input).taskId
    if(batch.rows.every(r=>r.taskId||r.cancelled))batch.view.dispatchState='finished'
  }
  const projects=(batch:Synthetic):BatchItems['items']=>batch.rows.map((row,index)=>{
    const task=row.taskId?load(row.taskId):undefined
    return{itemId:row.itemId,clientItemId:row.clientItemId,ordinal:index+1,input:row.input,dispatchState:row.cancelled?'cancelled':task?'accepted':'pending',state:row.cancelled?'cancelled':task?.state??'queued',task:task?structuredClone(task):null,error:null,canRerun:false,sourceItemId:null,sourceInvocationId:null,executionId:null}
  })
  const view=(batch:Synthetic)=>{
    advance(batch)
    const items=projects(batch),counts={total:items.length,queued:0,running:0,waitingInput:0,waitingApproval:0,completed:0,failed:0,cancelled:0,outcomeUnknown:0,ended:0}
    for(const item of items){const key=({accepted:'queued',queued:'queued',running:'running',waiting_input:'waitingInput',waiting_approval:'waitingApproval',completed:'completed',failed:'failed',cancelled:'cancelled',outcome_unknown:'outcomeUnknown'} as const)[item.state as TaskView['state']|'queued'];counts[key]+=1}
    counts.ended=counts.completed+counts.failed+counts.cancelled
    const state=counts.outcomeUnknown?'outcome_unknown':counts.queued+counts.running?'running':counts.waitingInput+counts.waitingApproval?'waiting':counts.failed?'partial_failure':counts.cancelled?'cancelled':'completed'
    return{...batch.view,counts,state,canStop:items.some(i=>i.dispatchState==='pending'),canCancel:items.some(i=>i.dispatchState==='pending'||i.task?.allowedActions.canCancel),revision:1+items.reduce((n,i)=>n+(i.task?.revision??0),0)}
  }
  const create=(args:Record<string,unknown>)=>{
    const inputs=args.items as Array<{clientItemId:string;input:unknown}>
    if(!inputs.length||inputs.length>100||new Set(inputs.map(i=>i.clientItemId)).size!==inputs.length)throw deny('Use 1–100 uniquely identified rows')
    const id=crypto.randomUUID(),batch:Synthetic={view:{batchId:id,applicationId:contract.applicationId,name:String(args.name),source:'web',callerType:'user',callerId:crypto.randomUUID(),callerName:'Preview user',workflowVersionNumber:1,bundleId:contract.bundleId,workflowVersionId:contract.workflowVersionId,accessBinding:null,sourceBatchId:null,dispatchState:'active',state:'running',revision:1,counts:{total:inputs.length,queued:inputs.length,running:0,waitingInput:0,waitingApproval:0,completed:0,failed:0,cancelled:0,outcomeUnknown:0,ended:0},createdAt:new Date().toISOString(),canStop:true,canCancel:true,canRerun:false},rows:inputs.map(i=>({itemId:crypto.randomUUID(),clientItemId:i.clientItemId,input:structuredClone(i.input)}))}
    counts.create++;counts.items+=inputs.length;submitted=structuredClone(inputs);batches.set(id,batch);latestId=id;return view(batch)
  }
  return{
    counts,handles:(method:PageMethod)=>methods.has(method),submitted:()=>submitted,latest:()=>latestId?view(owned(latestId)):undefined,
    dispatch(method:PageMethod,args:Record<string,unknown>):unknown{
      if(method==='createBatch')return create(args)
      if(method==='listMyBatches'){counts.historyListed++;return{items:[...batches.values()].map(b=>b.view.batchId===historyId?b.view:view(b)),next:null}}
      if(method==='readTable'){const input=sample(contract.inputSchema) as Record<string,unknown>;const headers=Object.keys(input);return{headers,rows:[headers.map(k=>typeof input[k]==='string'?input[k]:JSON.stringify(input[k]))],lines:[2]}}
      if(method==='openBatch'){owned(String(args.batchId),true);counts.historyOpened++;counts.lastHistoryOpened=String(args.batchId);return{simulated:true}}
      const batch=owned(String(args.batchId));advance(batch)
      switch(method){
        case 'getBatch':counts.observations++;counts.observedRevision=view(batch).revision;return view(batch)
        case 'listBatchItems':{
          counts.observations++;counts.observedRevision=view(batch).revision;const query=args.query as {limit?:number;cursor?:string;state?:string},all=projects(batch).filter(i=>!query.state||i.state===query.state),offset=Number(query.cursor??0),limit=query.limit??20
          const items=all.slice(offset,offset+limit);for(const item of items)if(item.task)observe(item.task)
          return{items,next:offset+limit<all.length?String(offset+limit):null}
        }
        case 'stopBatch':case 'cancelBatch':{
          batch.view.dispatchState='stopped';for(const row of batch.rows){if(!row.taskId)row.cancelled=true;else if(method==='cancelBatch'){const task=load(row.taskId);if(task.allowedActions.canCancel){task.state='cancelled';task.invocationStatus='cancelled';task.pendingInputs=[];task.allowedActions={canCancel:false,inputWaitIds:[],downloadableArtifactIds:[]};task.revision++}}}return view(batch)
        }
        case 'rerunFailedItems':throw deny('This synthetic scenario has no eligible failures')
        case 'exportBatch':{const id=crypto.randomUUID(),job:BatchExport={exportId:id,batchId:batch.view.batchId,status:'completed',file:{artifactId:crypto.randomUUID(),contentType:'text/csv',sizeBytes:128,sha256:'0'.repeat(64)},error:null};exports.set(id,job);return job}
        case 'getBatchExport':return exports.get(String(args.exportId))??(()=>{throw deny('Synthetic export was not found')})()
        case 'downloadBatchExport':return{simulated:true}
      }throw deny('Unsupported batch operation')
    },
    seedHistory(){const made=create({name:'Previous batch',items:[{clientItemId:'history',input:sample(contract.inputSchema)}]});historyId=made.batchId;const original=batches.get(historyId)!;original.view={...made,bundleId:crypto.randomUUID(),workflowVersionId:crypto.randomUUID(),state:'completed',counts:{...made.counts,total:1,completed:1,queued:0,ended:1}};latestId=undefined;counts.create=0;counts.items=0;submitted=[];return historyId},
    reset(){batches.clear();exports.clear();latestId=undefined;historyId=undefined;submitted=[];Object.assign(counts,{create:0,items:0,observations:0,observedRevision:0,historyListed:0,historyOpened:0,lastHistoryOpened:''})},
  }
}
