import {afterEach,expect,it,vi} from 'vitest'
import {applicationPageClient,currentAuthSession,invalidateAuthSession} from '../../../shared/api/client'
import {createBatchPageHost} from './page-batch-live'
import type {PublishedPage} from './page-types'

const view={historical:false,routeKey:'route',publication:{tenantId:'tenant',applicationId:'app',publicationId:'page',revision:2,slug:'entrance',contract:{bundleId:'bundle'}}} as PublishedPage
const accepted={batchId:'batch',applicationId:'app',bundleId:'bundle',pagePublicationId:'page',accessBinding:{kind:'web',publicationId:'page'},counts:{total:2}}
const input={name:'same values are separate rows',items:[{clientItemId:'row-a',input:{n:1}},{clientItemId:'row-b',input:{n:1}}]}
function host(page=view){return createBatchPageHost(page,'employee',applicationPageClient(currentAuthSession()),()=>new AbortController().signal,()=>{},vi.fn(),vi.fn(),vi.fn())}
afterEach(()=>{window.localStorage.clear();vi.unstubAllGlobals();invalidateAuthSession()})

it('recovers an accepted submission after losing both transport responses without opening a new intention',async()=>{
  const writes:string[]=[]
  vi.stubGlobal('fetch',vi.fn(async(_url:string,init:RequestInit)=>{
    if(init.method==='POST'){writes.push(new Headers(init.headers).get('Idempotency-Key')!);throw new TypeError('response lost')}
    return new Response(JSON.stringify({items:[accepted]}),{status:200})
  }))
  const first=host()
  await expect(first.dispatch('createBatch',input,'rpc-1')).rejects.toThrow()
  expect(writes).toHaveLength(2);expect(writes[0]).toBe(writes[1])
  first.close()
  const recovered=await host().dispatch('createBatch',input,'rpc-after-reload')
  expect(recovered).toEqual(accepted);expect(writes).toHaveLength(2)
  expect(window.localStorage.length).toBe(0)
})

it('retains the business key across a new transport ID while an earlier request is unresolved',async()=>{
  const writes:string[]=[]
  vi.stubGlobal('fetch',vi.fn(async(_url:string,init:RequestInit)=>{
    if(init.method==='POST'){writes.push(new Headers(init.headers).get('Idempotency-Key')!);if(writes.length<=2)throw new TypeError('offline');return new Response(JSON.stringify(accepted),{status:202})}
    return new Response(JSON.stringify({items:[]}),{status:200})
  }))
  await expect(host().dispatch('createBatch',input,'one')).rejects.toThrow()
  expect(await host().dispatch('createBatch',input,'two')).toEqual(accepted)
  expect(new Set(writes).size).toBe(1)
})

it('blocks changed input while the earlier business outcome cannot be confirmed',async()=>{
  let posts=0
  vi.stubGlobal('fetch',vi.fn(async(_url:string,init:RequestInit)=>{if(init.method==='POST'){posts++;throw new TypeError('offline')}return new Response(JSON.stringify({items:[]}),{status:200})}))
  await expect(host().dispatch('createBatch',input,'one')).rejects.toThrow()
  await expect(host().dispatch('createBatch',{...input,name:'changed'},'two')).rejects.toMatchObject({code:'BATCH_SUBMISSION_UNRESOLVED'})
  expect(posts).toBe(2)
})

it('denies new historical submissions and restores other-version batches only through openBatch',async()=>{
  const navigation=vi.fn()
  vi.stubGlobal('fetch',vi.fn(async()=>new Response(JSON.stringify({...accepted,bundleId:'original'}),{status:200})))
  const live=createBatchPageHost(view,'employee',applicationPageClient(currentAuthSession()),()=>new AbortController().signal,()=>{},navigation,vi.fn(),vi.fn())
  await expect(live.dispatch('getBatch',{batchId:'batch'},'read')).rejects.toMatchObject({code:'PAGE_BATCH_VERSION_MISMATCH'})
  await live.dispatch('openBatch',{batchId:'batch'},'open');expect(navigation).toHaveBeenCalledWith('?batchId=batch')
  await expect(host({...view,historical:true}).dispatch('createBatch',input,'new')).rejects.toMatchObject({code:'PAGE_CAPABILITY_DENIED'})
})

it('replays an unconfirmed safe rerun with its original business key after a reload',async()=>{
  const writes:string[]=[],navigation=vi.fn(),derived={...accepted,batchId:'derived',bundleId:'current-business'}
  vi.stubGlobal('fetch',vi.fn(async(_url:string,init:RequestInit)=>{
    if(init.method==='POST'){
      writes.push(new Headers(init.headers).get('Idempotency-Key')!)
      if(writes.length<=2)throw new TypeError('accepted response lost')
      return new Response(JSON.stringify(derived),{status:202})
    }
    return new Response(JSON.stringify(accepted),{status:200})
  }))
  const args={batchId:'batch',itemIds:['second','first']}
  await expect(host().dispatch('rerunFailedItems',args,'rpc-one')).rejects.toThrow()
  const restored=createBatchPageHost({...view,historical:true},'employee',applicationPageClient(currentAuthSession()),()=>new AbortController().signal,()=>{},navigation,vi.fn(),vi.fn())
  expect(await restored.dispatch('rerunFailedItems',{...args,itemIds:['first','second']},'rpc-after-reload')).toEqual(derived)
  expect(new Set(writes).size).toBe(1)
  expect(navigation).toHaveBeenCalledWith('?batchId=derived')
  expect(window.localStorage.length).toBe(0)
})

it('collapses repeat clicks after acceptance until a new page intention is opened',async()=>{
  const fetch=vi.fn(async()=>new Response(JSON.stringify(accepted),{status:202}))
  vi.stubGlobal('fetch',fetch)
  const live=host()
  expect(await live.dispatch('createBatch',input,'one')).toEqual(accepted)
  expect(await live.dispatch('createBatch',input,'second-click')).toEqual(accepted)
  expect(fetch).toHaveBeenCalledTimes(1)
})
