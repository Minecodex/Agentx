import { ApiClientError, applicationPageClient, jsonBody } from '../../../shared/api/client'
import type { PageMethod } from './page-bridge'
import type { PublishedPage, TaskView } from './page-types'
import type { components } from '../../../shared/api/generated-gateway'

export type BatchView = components['schemas']['BatchViewV1']
export type BatchItems = components['schemas']['BatchItemsPageV1']
export type BatchExport = components['schemas']['BatchExportV1']
type PendingIntent = { key:string; fingerprint:string; publicationId:string; revision:number; operation:'create'|'rerun' }
const methods = new Set<PageMethod>(['createBatch','listMyBatches','getBatch','listBatchItems','openBatch','stopBatch','cancelBatch','rerunFailedItems','exportBatch','getBatchExport','downloadBatchExport','readTable'])
const denied = (code:string,message:string)=>Object.assign(new Error(message),{code})

/** Only intent metadata is retained; row inputs and credentials never enter browser storage. */
export function createBatchPageHost(view:PublishedPage,actorId:string,client:ReturnType<typeof applicationPageClient>,signal:()=>AbortSignal,requireOpen:()=>void,navigate:(path:string)=>void,created:(id:string)=>void,seen:(task:TaskView)=>void){
  const publication=view.publication
  const storageKey=`agentx.batch-intent:${publication.tenantId}:${actorId}:${publication.applicationId}`
  const downloads=new Set<string>()
  const completed=new Map<string,BatchView>()
  const digest=async(value:unknown)=>{
    const bytes=await crypto.subtle.digest('SHA-256',new TextEncoder().encode(JSON.stringify(value)))
    return Array.from(new Uint8Array(bytes),n=>n.toString(16).padStart(2,'0')).join('')
  }
  const headers={'x-agentx-page-publication':publication.publicationId,'x-agentx-page-revision':String(publication.revision)}
  const newOperation=()=>{requireOpen();if(view.historical)throw denied('PAGE_CAPABILITY_DENIED','Open the current application page to create a new batch')}
  const write=async<T>(path:string,body:unknown,key:string):Promise<T>=>{
    for(let attempt=0;;attempt++){
      requireOpen()
      try{return await client.request<T>(path,{method:'POST',headers:{...headers,'Idempotency-Key':key},body:body===undefined?undefined:jsonBody(body),signal:signal()})}
      catch(error){if(attempt>=1||!(error instanceof ApiClientError)||![0,502,503,504].includes(error.status))throw error}
    }
  }
  const load=async(id:string,otherVersion=false)=>{
    requireOpen();const batch=await client.request<BatchView>(`/batches/${id}`,{signal:signal()})
    if(batch.applicationId!==publication.applicationId)throw denied('PAGE_CAPABILITY_DENIED','This batch belongs to another application')
    if(!otherVersion&&(batch.bundleId!==publication.contract.bundleId||batch.pagePublicationId!==publication.publicationId))throw denied('PAGE_BATCH_VERSION_MISMATCH','Open this batch in its original application page')
    return batch
  }
  const create=async(args:Record<string,unknown>)=>{
    newOperation()
    if(!actorId)throw denied('PAGE_CAPABILITY_DENIED','A trusted signed-in subject is required for batch submission')
    const fingerprint=await digest({operation:'create',args})
    if(completed.has(fingerprint))return completed.get(fingerprint)!
    const raw=window.localStorage.getItem(storageKey)
    const saved:PendingIntent|undefined=raw?JSON.parse(raw):undefined
    if(saved){
      if(saved.operation!=='create')throw denied('BATCH_SUBMISSION_UNRESOLVED','Confirm the previous rerun before creating another batch')
      const prior=await client.request<{items:BatchView[]}>(`/applications/${view.routeKey}/batches?${new URLSearchParams({requestKey:saved.key,limit:'1'})}`,{signal:signal()})
      if(prior.items.length){
        window.localStorage.removeItem(storageKey)
        if(saved.fingerprint!==fingerprint)throw denied('BATCH_PREVIOUS_SUBMISSION_FOUND','The previous submission was accepted. Review My batches, then explicitly submit the new data.')
        const original=prior.items[0]!;completed.set(fingerprint,original);created(original.batchId)
        if(original.bundleId!==publication.contract.bundleId||saved.publicationId!==publication.publicationId){navigate(`?batchId=${original.batchId}`);throw denied('PAGE_BATCH_VERSION_MISMATCH','The accepted batch is opening in its original page')}
        return original
      }
      if(saved.fingerprint!==fingerprint)throw denied('BATCH_SUBMISSION_UNRESOLVED','The previous submission has not been confirmed. Restore its input or check My batches before submitting different data.')
    }
    const intent:PendingIntent=saved??{key:`batch:${crypto.randomUUID()}`,fingerprint,publicationId:publication.publicationId,revision:publication.revision,operation:'create'}
    window.localStorage.setItem(storageKey,JSON.stringify(intent))
    try{
      const batch=await write<BatchView>(`/pages/${publication.slug}/batches`,{...args,publicationId:intent.publicationId,revision:intent.revision},intent.key)
      window.localStorage.removeItem(storageKey);completed.set(fingerprint,batch);created(batch.batchId);return batch
    }catch(error){if(error instanceof ApiClientError&&[400,409,413,422].includes(error.status))window.localStorage.removeItem(storageKey);throw error}
  }
  const rerun=async(args:Record<string,unknown>)=>{
    requireOpen()
    if(!actorId)throw denied('PAGE_CAPABILITY_DENIED','A trusted signed-in subject is required')
    const itemIds=[...(args.itemIds as string[])].sort(),batchId=String(args.batchId)
    const fingerprint=await digest({operation:'rerun',batchId,itemIds})
    if(completed.has(fingerprint))return completed.get(fingerprint)!
    const raw=window.localStorage.getItem(storageKey),saved:PendingIntent|undefined=raw?JSON.parse(raw):undefined
    if(saved&&(saved.operation!=='rerun'||saved.fingerprint!==fingerprint))throw denied('BATCH_SUBMISSION_UNRESOLVED','Confirm the original submission before changing the selected failed items')
    const intent:PendingIntent=saved??{operation:'rerun',key:`rerun:${crypto.randomUUID()}`,fingerprint,publicationId:publication.publicationId,revision:publication.revision}
    window.localStorage.setItem(storageKey,JSON.stringify(intent))
    try{
      const batch=await write<BatchView>(`/batches/${batchId}/rerun`,{itemIds},intent.key)
      window.localStorage.removeItem(storageKey);completed.set(fingerprint,batch)
      // Runtime authorizes and binds the new batch to the current publication;
      // the historical frame never renders it using its own frozen contract.
      navigate(`?batchId=${batch.batchId}`);return batch
    }catch(error){if(error instanceof ApiClientError&&[400,409,413,422].includes(error.status))window.localStorage.removeItem(storageKey);throw error}
  }
  return {
    handles:(method:PageMethod)=>methods.has(method),
    async dispatch(method:PageMethod,args:Record<string,unknown>,id:string):Promise<unknown>{
      requireOpen()
      if(method==='createBatch')return create(args)
      if(method==='listMyBatches'){
        const params=new URLSearchParams();for(const[key,value]of Object.entries(args.query as object))if(value!==undefined)params.set(key,String(value))
        return client.request(`/applications/${view.routeKey}/batches?${params}`,{signal:signal()})
      }
      if(method==='readTable')return write(`/applications/${view.routeKey}/batch-imports/parse`,args,`table:${id}`)
      const batchId=String(args.batchId)
      if(method==='openBatch'){await load(batchId,true);navigate(`?batchId=${batchId}`);return null}
      const batch=await load(batchId)
      switch(method){
        case 'getBatch':return batch
        case 'listBatchItems':{
          const params=new URLSearchParams();for(const[key,value]of Object.entries(args.query as object))if(value!==undefined)params.set(key,String(value))
          const page=await client.request<BatchItems>(`/batches/${batchId}/items?${params}`,{signal:signal()})
          for(const item of page.items)if(item.task)seen(item.task)
          return page
        }
        case 'stopBatch':return write(`/batches/${batchId}/stop`,undefined,`batch-stop:${id}`)
        case 'cancelBatch':return write(`/batches/${batchId}/cancel`,undefined,`batch-cancel:${id}`)
        case 'rerunFailedItems':return rerun(args)
        case 'exportBatch':return write(`/batches/${batchId}/exports`,undefined,`batch-export:${id}`)
        case 'getBatchExport':return client.request<BatchExport>(`/batches/${batchId}/exports/${String(args.exportId)}`,{signal:signal()})
        case 'downloadBatchExport':{
          const blob=await client.blob(`/batches/${batchId}/exports/${String(args.exportId)}/file`,{signal:signal()});requireOpen()
          const url=URL.createObjectURL(blob);downloads.add(url);const link=document.createElement('a');link.href=url;link.download=`batch-${batchId}.csv`;link.click()
          setTimeout(()=>{URL.revokeObjectURL(url);downloads.delete(url)},1000);return null
        }
      }
      throw denied('PAGE_CAPABILITY_DENIED','Unsupported batch operation')
    },
    close(){for(const url of downloads)URL.revokeObjectURL(url);downloads.clear()},
  }
}
