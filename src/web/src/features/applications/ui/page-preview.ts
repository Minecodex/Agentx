import { createBatchPreview } from './page-batch-preview'
import type { PageContract, TaskView } from './page-types'
import type { PageDispatcher } from './page-bridge'
import { Validator, type Schema } from '@cfworker/json-schema'

function validateInput(schema:unknown,value:unknown,operation:string){
  // The interpreter needs no eval/Function and cannot fetch remote schemas.
  // It annotates its schema lookup, so never give it the frozen contract itself.
  const result=new Validator(structuredClone(schema) as Schema|boolean,'2020-12',false).validate(value)
  if(!result.valid)throw Object.assign(new Error(`${operation}: ${result.errors.slice(0,5).map(error=>`${error.instanceLocation} ${error.error}`).join('; ').slice(0,1600)}`),{code:'PREVIEW_INPUT_SCHEMA_INVALID'})
}

/** Synthetic values are never submitted to Runtime and never treated as business evidence. */
export function sampleValue(raw: unknown): unknown {
  const schema=(raw&&typeof raw==='object'?raw:{}) as Record<string,unknown>
  if(schema.const!==undefined)return schema.const
  if(Array.isArray(schema.enum))return schema.enum[0]
  if(schema.default!==undefined)return structuredClone(schema.default)
  for(const key of ['oneOf','anyOf'])if(Array.isArray(schema[key]))return sampleValue((schema[key] as unknown[])[0])
  const type=Array.isArray(schema.type)?schema.type.find(value=>value!=='null'):schema.type
  if(type==='object'||schema.properties){const result:Record<string,unknown>={};for(const [name,child] of Object.entries((schema.properties??{}) as object))result[name]=sampleValue(child);return result}
  if(type==='array')return Array.from({length:Math.max(Number(schema.minItems??0),Math.min(2,Number(schema.maxItems??2)))},()=>sampleValue(schema.items))
  if(type==='boolean')return true
  if(type==='number'||type==='integer'){
    const minimum=Number(schema.minimum??schema.exclusiveMinimum??1)+(schema.exclusiveMinimum!==undefined?1:0)
    const maximum=Number(schema.maximum??minimum+100),step=Number(schema.multipleOf??1)
    const first=Math.ceil(minimum/step)*step
    if(type==='number'){
      const fractional=schema.multipleOf===undefined?minimum+Math.min(0.5,(maximum-minimum)/2):first+(Number.isInteger(first)&&!Number.isInteger(step)?step:0)
      if(fractional>=minimum&&fractional<=maximum&&!Number.isInteger(fractional))return fractional
    }
    return Math.min(maximum,first)
  }
  if(schema.format==='uuid')return crypto.randomUUID()
  if(schema.format==='date')return '2026-11-02'
  if(schema.format==='date-time')return '2026-11-02T09:00:00Z'
  if(schema.format==='email')return 'preview@example.test'
  return 'Preview'.padEnd(Math.min(100,Number(schema.minLength??0)),'x').slice(0,Number(schema.maxLength??100))
}

export function createPagePreview(contract:PageContract,contractHash:string,locale:string,theme:string) {
  const tasks=new Map<string,TaskView>()
  const sessions=new Map<string,{id:string;messages:unknown[]}>()
  const chat={created:0,sent:0,loaded:0,directInvocations:0}
  let currentTaskId:string|null=null
  let currentBatchId:string|null=null
  const session=(id:string)=>{
    const value=sessions.get(id)
    if(!contract.chatMapping||!value)throw Object.assign(new Error('Use a formal session created for this chat application'),{code:'PAGE_SESSION_INVALID'})
    return value
  }
  const message=(id:string,taskId:string,role:string,content:unknown)=>{
    const value=session(id)
    value.messages.push({id:crypto.randomUUID(),invocationId:taskId,sequence:value.messages.length+1,role,parts:[{partType:'text',content,artifactId:null}],createdAt:new Date().toISOString()})
  }
  let maxSubscriptions=0
  const subscribers=new Map<string,ReturnType<typeof setInterval>>()
  const transitions=new Set<ReturnType<typeof setTimeout>>()
  const observed=new Map<string,string>()
  const revisions=new Map<string,number>()
  const counts={invoke:0,input:0,upload:0,formalRequests:0}
  const history={listed:0,opened:0,lastOpened:null as string|null,taskId:null as string|null}
  let latest:TaskView|undefined
  let submittedInput:unknown
  const input=sampleValue(contract.inputSchema)
  const load=(taskId:string)=>{const value=tasks.get(taskId);if(!value)throw Object.assign(new Error('Synthetic task was not found'),{code:'TASK_NOT_FOUND'});return value}
  const newTask=():TaskView=>({taskId:crypto.randomUUID(),taskUrl:null,applicationId:contract.applicationId,deploymentId:contract.deploymentId,workflowVersionId:contract.workflowVersionId,bundleId:contract.bundleId,sessionId:null,callerType:'user',accessBinding:null,state:'accepted',invocationStatus:'queued',stages:[],pendingInputs:[],pendingApprovals:[],messages:[],outputs:null,files:[],error:null,allowedActions:{canCancel:true,inputWaitIds:[],downloadableArtifactIds:[]},revision:1,eventCursor:1,createdAt:new Date().toISOString()})
  const observe=(task:TaskView)=>{observed.set(task.taskId,task.state);revisions.set(task.taskId,task.revision)}
  const invoke=(sessionId:string|null=null,logical=true)=>{
    if(logical)counts.invoke+=1
    const task=newTask(),taskId=task.taskId
    task.sessionId=sessionId
    const waits=contract.waitInputNodes.map(node=>({waitId:crypto.randomUUID(),executionId:crypto.randomUUID(),nodeExecutionId:crypto.randomUUID(),nodeId:node.nodeId,nodePath:node.path,occurrence:1,status:'waiting',responseSchema:node.responseSchema,resumeUrl:null,expiresAt:null,content:null,source:null}))
    tasks.set(taskId,task);latest=task
    const timer=setTimeout(()=>{
      transitions.delete(timer)
      if(task.state==='cancelled')return
      task.state=waits.length?'waiting_input':'completed';task.invocationStatus=waits.length?'running':'completed'
      task.pendingInputs=waits;task.outputs=waits.length?null:sampleValue(contract.outputSchema)
      if(!waits.length&&sessionId)message(sessionId,taskId,'assistant',(task.outputs as Record<string,unknown>)?.[contract.chatMapping!.answerOutput])
      task.allowedActions.canCancel=waits.length>0;task.allowedActions.inputWaitIds=waits.map(wait=>wait.waitId)
      task.revision+=1;task.eventCursor+=1
    },200)
    transitions.add(timer)
    return structuredClone(task)
  }
  const batches=createBatchPreview(contract,input=>{validateInput(contract.inputSchema,input,'batch.input');return invoke(null,false)},load,observe,sampleValue)
  const dispatch:PageDispatcher=async(method,args,_requestId,notify)=>{
    if(batches.handles(method))return batches.dispatch(method,args)
    switch(method){
      case 'getContext':return{mode:'preview',contract,contractHash,locale,theme,user:{displayName:locale.startsWith('zh')?'预览用户':'Preview user'},taskId:currentTaskId,batchId:currentBatchId,capabilities:{batch:true,maxBatchItems:100,maxBatchBytes:1048576,batchPageLimit:50}}
      case 'invoke':validateInput(contract.inputSchema,args.input,'invoke.input');submittedInput=structuredClone(args.input);chat.directInvocations+=1;return invoke()
      case 'getTask':{
        const task=load(String(args.taskId))
        if(task.taskId===history.taskId)throw Object.assign(new Error('Open this history entry through openTask in its original application page'),{code:'PAGE_TASK_VERSION_MISMATCH'})
        observe(task);return structuredClone(task)
      }
      case 'listMyTasks':{
        history.listed+=1
        const query=(args.query??{}) as {activeOnly?:boolean;limit?:number}
        return{items:Array.from(tasks.values()).reverse().filter(task=>!query.activeOnly||!['completed','failed','cancelled','outcome_unknown'].includes(task.state)).slice(0,query.limit??50),next:null}
      }
      case 'submitTaskInput':{
        const task=load(String(args.taskId));const index=task.pendingInputs.findIndex(wait=>wait.waitId===args.waitId)
        if(index<0)throw Object.assign(new Error('Synthetic wait is no longer active'),{code:'WAIT_ALREADY_FINISHED'})
        validateInput(task.pendingInputs[index]!.responseSchema??true,args.content,'submitTaskInput.content')
        task.pendingInputs.splice(index,1);counts.input+=1;task.revision+=1;task.eventCursor+=1
        task.allowedActions.inputWaitIds=task.pendingInputs.map(wait=>wait.waitId)
        if(!task.pendingInputs.length){task.state='completed';task.invocationStatus='completed';task.outputs=sampleValue(contract.outputSchema);task.allowedActions.canCancel=false;if(task.sessionId)message(task.sessionId,task.taskId,'assistant',(task.outputs as Record<string,unknown>)?.[contract.chatMapping!.answerOutput])}
        return structuredClone(task)
      }
      case 'cancelTask':{const task=load(String(args.taskId));if(!task.allowedActions.canCancel)throw Object.assign(new Error('Synthetic task cannot be cancelled'),{code:'TASK_NOT_CANCELLABLE'});task.state='cancelled';task.invocationStatus='cancelled';task.pendingInputs=[];task.revision+=1;task.eventCursor+=1;task.allowedActions={canCancel:false,inputWaitIds:[],downloadableArtifactIds:[]};return structuredClone(task)}
      case 'uploadFile':counts.upload+=1;return{artifactId:crypto.randomUUID(),contentType:args.contentType,sizeBytes:(args.bytes as ArrayBuffer).byteLength,sha256:'0'.repeat(64)}
      case 'openTask':load(String(args.taskId));history.opened+=1;history.lastOpened=String(args.taskId);return{simulated:true}
      case 'downloadFile':case 'openApproval':return{simulated:true}
      case 'subscribeTask':{
        const task=load(String(args.taskId)),id=String(args.subscriptionId)
        if(task.taskId===history.taskId)throw Object.assign(new Error('Open this history entry through openTask in its original application page'),{code:'PAGE_TASK_VERSION_MISMATCH'})
        if(subscribers.size>=16)throw Object.assign(new Error('Too many preview subscriptions'),{code:'PAGE_BUSY'})
        const existing=subscribers.get(id);if(existing)clearInterval(existing)
        let revision=0
        const update=()=>{if(revision===task.revision)return;revision=task.revision;observe(task);notify(id,structuredClone(task))}
        subscribers.set(id,setInterval(update,100));maxSubscriptions=Math.max(maxSubscriptions,subscribers.size)
        update();return{simulated:true}
      }
      case 'unsubscribeTask':{const id=String(args.subscriptionId),timer=subscribers.get(id);if(timer)clearInterval(timer);subscribers.delete(id);return{simulated:true}}
      case 'createSession':{
        if(!contract.chatMapping)throw Object.assign(new Error('This application has no declared chat mapping'),{code:'PAGE_CHAT_NOT_ENABLED'})
        const id=crypto.randomUUID();sessions.set(id,{id,messages:[]});chat.created+=1;return{id}
      }
      case 'getSession':{chat.loaded+=1;return structuredClone(session(String(args.sessionId)))}
      case 'sendMessage':{
        const value=session(String(args.sessionId))
        if(typeof args.text!=='string'||!args.text.trim())throw Object.assign(new Error('A chat message is required'),{code:'PREVIEW_CHAT_MESSAGE_INVALID'})
        const task=invoke(value.id);chat.sent+=1;message(value.id,task.taskId,'user',args.text);return task
      }
    }
  }
  const close=()=>{for(const timer of subscribers.values())clearInterval(timer);subscribers.clear();for(const timer of transitions)clearTimeout(timer);transitions.clear()}
  return{dispatch,input,contract,counts,history,chat,batches,maxTaskSubscriptions:()=>maxSubscriptions,submittedInput:()=>submittedInput,latest:()=>latest,observedState:(taskId:string)=>observed.get(taskId),observedRevision:(taskId:string)=>revisions.get(taskId),
  seedHistory(){
    const task=newTask();task.state='completed';task.invocationStatus='completed';task.outputs=sampleValue(contract.outputSchema);task.allowedActions.canCancel=false
    task.bundleId=crypto.randomUUID();task.workflowVersionId=crypto.randomUUID();task.accessBinding={kind:'web',publicationId:crypto.randomUUID(),revision:1}
    tasks.set(task.taskId,task);history.taskId=task.taskId;return task.taskId
  },updateWaitingTask(taskId:string){
    const task=load(taskId)
    if(!task.pendingInputs.length)throw new Error('Only an active synthetic wait may receive a preview update')
    task.messages.push({id:crypto.randomUUID(),executionId:crypto.randomUUID(),nodeExecutionId:crypto.randomUUID(),content:'Synthetic preview status update',createdAt:new Date().toISOString()});task.revision+=1;task.eventCursor+=1;return task.revision
  },reopen(taskId:string){
    load(taskId);currentTaskId=taskId;currentBatchId=null;observed.delete(taskId);revisions.delete(taskId)
    for(const timer of subscribers.values())clearInterval(timer);subscribers.clear()
  },reopenBatch(batchId:string){
    batches.dispatch('getBatch',{batchId});currentBatchId=batchId;currentTaskId=null
    for(const timer of subscribers.values())clearInterval(timer);subscribers.clear()
  },close,reset(){
    close();batches.reset();maxSubscriptions=0;tasks.clear();sessions.clear();observed.clear();revisions.clear();latest=undefined;submittedInput=undefined;currentTaskId=null;currentBatchId=null
    Object.assign(chat,{created:0,sent:0,loaded:0,directInvocations:0})
    Object.assign(counts,{invoke:0,input:0,upload:0,formalRequests:0})
    Object.assign(history,{listed:0,opened:0,lastOpened:null,taskId:null})
  }}
}
