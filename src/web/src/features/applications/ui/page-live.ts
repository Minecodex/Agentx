import { applicationPageClient, ApiClientError, jsonBody } from '../../../shared/api/client'
import { consumeServerSentEvents } from '../../../shared/lib/server-sent-events'
import type { PageDispatcher } from './page-bridge'
import type { PublishedPage, TaskView } from './page-types'

const terminal=new Set(['completed','failed','cancelled','outcome_unknown'])
const denied=(message:string)=>Object.assign(new Error(message),{code:'PAGE_CAPABILITY_DENIED'})

/** Only this trusted host knows the account and application binding. No URLs or credentials enter the frame. */
export function createLivePage(view:PublishedPage,session:number,locale:string,theme:string,displayName:string,navigate:(path:string)=>void,created:(taskId:string)=>void,onError:(error:Error)=>void){
  const client=applicationPageClient(session)
  const controller=new AbortController()
  const subscriptions=new Map<string,AbortController>()
  const approvals=new Set<string>()
  const downloads=new Set<string>()
  const publication=view.publication
  const application=publication.applicationId
  const headers={'x-agentx-page-publication':publication.publicationId,'x-agentx-page-revision':String(publication.revision)}
  let closed=false
  const signal=()=>AbortSignal.any([controller.signal,AbortSignal.timeout(25_000)])
  const requireOpen=()=>{if(closed)throw denied('This application page is closed')}
  const newOperation=()=>{requireOpen();if(view.historical)throw denied('Open the current application page to start a new task')}
  const load=async(taskId:string,otherVersion=false)=>{
    requireOpen()
    const task=await client.request<TaskView>(`/tasks/${taskId}`,{signal:signal()})
    if(task.applicationId!==application)throw denied('This task belongs to another application')
    if(!otherVersion&&(task.bundleId!==publication.contract.bundleId||(task.accessBinding?.kind==='web'&&task.accessBinding.publicationId!==publication.publicationId)))throw Object.assign(new Error('Open this history entry in its original application page'),{code:'PAGE_TASK_VERSION_MISMATCH'})
    for(const approval of task.pendingApprovals)approvals.add(approval.approvalId)
    return task
  }
  const write=async<T>(path:string,body:unknown,id:string,multipart=false):Promise<T>=>{
    const digest=await crypto.subtle.digest('SHA-256',new TextEncoder().encode(`${publication.publicationId}:${id}`))
    const key=`page:${Array.from(new Uint8Array(digest)).map(n=>n.toString(16).padStart(2,'0')).join('')}`
    for(let attempt=0;;attempt+=1){
      requireOpen()
      try{return await client.request<T>(path,{method:'POST',headers:{...headers,'Idempotency-Key':key},body:multipart?body as FormData:jsonBody(body),signal:signal()})}
      catch(error){if(closed||attempt>=1||!(error instanceof ApiClientError)||![0,502,503,504].includes(error.status))throw error}
    }
  }
  const conversation=()=>{if(!publication.contract.chatMapping)throw denied('This application does not declare a conversation contract')}
  const sessionInfo=async(id:string)=>{
    conversation()
    const value=await client.request<{id:string;applicationId:string;bundleId:string|null}>(`/sessions/${id}`,{signal:signal()})
    if(value.applicationId!==application||value.bundleId!==publication.contract.bundleId)throw denied('The conversation belongs to another application or business version')
    return value
  }
  const dispatch:PageDispatcher=async(method,args,id,notify)=>{
    requireOpen()
    switch(method){
      case 'getContext':return{mode:view.historical?'history':'live',contract:publication.contract,contractHash:publication.contractHash,locale,theme,user:{displayName},taskId:view.taskId}
      case 'invoke':{
        newOperation()
        const task=await write<TaskView>(`/pages/${publication.slug}/invocations`,{publicationId:publication.publicationId,revision:publication.revision,input:args.input},id)
        if(task.applicationId!==application||task.bundleId!==publication.contract.bundleId)throw denied('The accepted task does not match this page version')
        created(task.taskId);return load(task.taskId)
      }
      case 'getTask':return load(String(args.taskId))
      case 'listMyTasks':{
        const query=args.query as {limit?:number;cursor?:string;activeOnly?:boolean}
        const params=new URLSearchParams();for(const [key,value]of Object.entries(query))if(value!==undefined)params.set(key,String(value))
        const page=await client.request<{items:TaskView[];next:string|null}>(`/applications/${view.routeKey}/my-tasks?${params}`,{signal:signal()})
        for(const task of page.items){if(task.applicationId!==application)throw denied('Task history belongs to another application');for(const approval of task.pendingApprovals)approvals.add(approval.approvalId)}
        return page
      }
      case 'submitTaskInput':{
        const task=await load(String(args.taskId))
        if(!task.allowedActions.inputWaitIds.includes(String(args.waitId)))throw denied('This input wait is no longer available')
        await write(`/invocations/${task.taskId}/waits/${args.waitId}/input`,{content:args.content},id)
        return load(task.taskId)
      }
      case 'cancelTask':{const task=await load(String(args.taskId));if(!task.allowedActions.canCancel)throw denied('This task cannot be cancelled');await write(`/invocations/${task.taskId}/cancel`,{},id);return load(task.taskId)}
      case 'uploadFile':{
        const form=new FormData();form.append('file',new Blob([args.bytes as ArrayBuffer],{type:String(args.contentType||'application/octet-stream')}),String(args.name))
        return write('/artifacts',form,id,true)
      }
      case 'downloadFile':{
        const task=await load(String(args.taskId)),artifact=String(args.artifactId)
        if(!task.allowedActions.downloadableArtifactIds.includes(artifact))throw denied('This file is not an output of the authorized task')
        const blob=await client.blob(`/invocations/${task.taskId}/artifacts/${artifact}`,{signal:signal()});requireOpen()
        const url=URL.createObjectURL(blob);downloads.add(url);const link=document.createElement('a');link.href=url;link.download=artifact;link.click()
        setTimeout(()=>{URL.revokeObjectURL(url);downloads.delete(url)},1000);return null
      }
      case 'openTask':{const task=await load(String(args.taskId),true);navigate(`?taskId=${task.taskId}`);return null}
      case 'openApproval':{
        const approval=String(args.approvalId)
        if(!approvals.has(approval))throw denied('Load the original task before opening its approval')
        navigate(`/approvals/${approval}`);return null
      }
      case 'subscribeTask':{
        const subscriptionId=String(args.subscriptionId),taskId=String(args.taskId)
        if(subscriptions.size>=8&&!subscriptions.has(subscriptionId))throw denied('The page subscription limit was reached')
        subscriptions.get(subscriptionId)?.abort()
        const watch=new AbortController();subscriptions.set(subscriptionId,watch)
        const first=await load(taskId);notify(subscriptionId,first)
        void(async()=>{
          let task=first
          while(!closed&&!watch.signal.aborted&&!terminal.has(task.state)){
            try{
              const response=await client.stream(`/invocations/${taskId}/events`,{headers:{'Last-Event-ID':String(task.eventCursor)},signal:AbortSignal.any([controller.signal,watch.signal,AbortSignal.timeout(60_000)])})
              await consumeServerSentEvents(response,async()=>{if(!closed&&!watch.signal.aborted){task=await load(taskId);notify(subscriptionId,task)}})
              task=await load(taskId);if(!watch.signal.aborted)notify(subscriptionId,task)
            }catch(error){
              if(closed||watch.signal.aborted)break
              if(error instanceof ApiClientError&&[401,403,404].includes(error.status)){onError(error);break}
              await new Promise(resolve=>setTimeout(resolve,1000))
              try{task=await load(taskId);notify(subscriptionId,task)}catch(error){onError(error as Error);break}
            }
          }
          if(subscriptions.get(subscriptionId)===watch)subscriptions.delete(subscriptionId)
        })()
        return null
      }
      case 'unsubscribeTask':{const key=String(args.subscriptionId);subscriptions.get(key)?.abort();subscriptions.delete(key);return null}
      case 'createSession':{
        newOperation();conversation()
        return write(`/applications/${view.routeKey}/sessions`,{input:args.input,title:publication.contract.applicationName},id)
      }
      case 'getSession':{const value=await sessionInfo(String(args.sessionId));const messages=await client.request<unknown[]>(`/sessions/${value.id}/messages`,{signal:signal()});return{...value,messages}}
      case 'sendMessage':{
        newOperation();const value=await sessionInfo(String(args.sessionId))
        const parts=[{partType:'text',content:args.text},...(args.files as Array<{artifactId:string}>).map(file=>({partType:'file',artifactId:file.artifactId}))]
        const accepted=await write<{id:string}>(`/sessions/${value.id}/messages`,{parts},id)
        created(accepted.id);return load(accepted.id)
      }
    }
  }
  return{dispatch,close(){closed=true;controller.abort();for(const watcher of subscriptions.values())watcher.abort();subscriptions.clear();for(const url of downloads)URL.revokeObjectURL(url);downloads.clear()}}
}
