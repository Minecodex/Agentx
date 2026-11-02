import {afterEach,expect,it,vi} from 'vitest'
import {currentAuthSession,invalidateAuthSession} from '../../../shared/api/client'
import {createLivePage} from './page-live'
import type {PublishedPage} from './page-types'

afterEach(()=>{vi.unstubAllGlobals();invalidateAuthSession()})

it('opens another historical task through the current entrance after the original suffix changed',async()=>{
  const tasks=Object.fromEntries(['first','second'].map(id=>[id,{taskId:id,applicationId:'application',bundleId:'older-bundle',accessBinding:{kind:'web',publicationId:'older-publication'},pendingApprovals:[]}]))
  vi.stubGlobal('fetch',vi.fn(async(path:string)=>new Response(JSON.stringify(tasks[path.split('/').at(-1)!]),{status:200})))
  const view={historical:true,taskId:'first',routeKey:'business-route',publication:{applicationId:'application',publicationId:'older-publication',slug:'retired-entrance',revision:1,contract:{bundleId:'older-bundle'}}} as PublishedPage
  let entrance=new URL('https://agentx.test/a/current-entrance?taskId=first')
  const navigate=vi.fn((path:string)=>{entrance=new URL(path,entrance)})
  const live=createLivePage(view,currentAuthSession(),'zh-CN','light','Employee',navigate,vi.fn(),vi.fn())
  try{
    await live.dispatch('openTask',{taskId:'second'},'open',vi.fn())
    expect(entrance.pathname).toBe('/a/current-entrance')
    expect(entrance.searchParams.get('taskId')).toBe('second')
    await live.dispatch('openTask',{taskId:'first'},'back',vi.fn())
    expect(entrance.pathname).toBe('/a/current-entrance')
    expect(entrance.searchParams.get('taskId')).toBe('first')
    expect(navigate).toHaveBeenCalledTimes(2)
  }finally{live.close()}
})
