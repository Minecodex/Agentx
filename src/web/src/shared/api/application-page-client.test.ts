import { afterEach, expect, it, vi } from 'vitest'
import { applicationPageClient, currentAuthSession, invalidateAuthSession, setAccessToken, setRefreshHandler } from './client'

afterEach(()=>{vi.unstubAllGlobals();setAccessToken();setRefreshHandler(()=>Promise.resolve(false));invalidateAuthSession()})

it('retries a refreshed token within the same account and preserves the request key',async()=>{
  setAccessToken('first-fixture-token')
  const requests:Array<{token:string|null;key:string|null}>=[]
  vi.stubGlobal('fetch',vi.fn(async(_url,init:RequestInit)=>{
    const headers=new Headers(init.headers);requests.push({token:headers.get('Authorization'),key:headers.get('Idempotency-Key')})
    return requests.length===1?new Response('{}',{status:401}):new Response('{"accepted":true}',{status:200})
  }))
  setRefreshHandler(async()=>{setAccessToken('renewed-fixture-token');return true})
  const result=await applicationPageClient(currentAuthSession()).request('/pages/fixture/invocations',{method:'POST',headers:{'Idempotency-Key':'same-logical-request'},body:'{}'})
  expect(result).toEqual({accepted:true})
  expect(requests).toEqual([{token:'Bearer first-fixture-token',key:'same-logical-request'},{token:'Bearer renewed-fixture-token',key:'same-logical-request'}])
})

it('a page from the previous account cannot dispatch after a 401 refresh changes the login',async()=>{
  const fetch=vi.fn(async()=>new Response('{}',{status:401}));vi.stubGlobal('fetch',fetch)
  const client=applicationPageClient(currentAuthSession())
  setRefreshHandler(async()=>{invalidateAuthSession();setAccessToken('different-account-fixture');return true})
  await expect(client.request('/pages/fixture/invocations',{method:'POST',body:'{}'})).rejects.toMatchObject({detail:{code:'AUTH_SESSION_CHANGED'}})
  expect(fetch).toHaveBeenCalledTimes(1)
  await expect(client.request('/tasks/another-task')).rejects.toMatchObject({detail:{code:'AUTH_SESSION_CHANGED'}})
  expect(fetch).toHaveBeenCalledTimes(1)
})

it('does not deliver an in-flight response to a page after logout',async()=>{
  let complete:((response:Response)=>void)|undefined
  vi.stubGlobal('fetch',vi.fn(()=>new Promise<Response>(resolve=>{complete=resolve})))
  const pending=applicationPageClient(currentAuthSession()).request('/tasks/fixture')
  const rejected=expect(pending).rejects.toMatchObject({detail:{code:'AUTH_SESSION_CHANGED'}})
  invalidateAuthSession();complete!(new Response('{"outputs":"previous account"}'))
  await rejected
})
