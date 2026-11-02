import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { StrictMode, forwardRef, useImperativeHandle } from 'react'
import { beforeEach, expect, it, vi } from 'vitest'

import { apiRequest } from '../../../shared/api/client'
import { PageEditor } from './page-editor'
import type { PageDispatcher } from './page-bridge'
import type { PageFrameHandle } from './page-types'
import type { PageDraft } from './page-types'

const hash=`sha256:${'0'.repeat(64)}`
const frameScenario=vi.hoisted(()=>({active:false,activeRun:false,screen:'form',restarts:0,inFlight:0,maxInFlight:0}))
vi.mock('../../../shared/api/client',()=>({apiRequest:vi.fn(),jsonBody:JSON.stringify}))
vi.mock('../../../app/providers/auth-provider',()=>({useAuth:()=>({user:{id:'author',companyId:'company'}})}))
vi.mock('../../../shared/components/use-design-agent-events',()=>({useDesignAgentEvents:()=>undefined}))
vi.mock('./page-publication-panel',()=>({PagePublicationPanel:()=>null}))
vi.mock('./page-frame',()=>({PageFrame:forwardRef<PageFrameHandle,{dispatch:PageDispatcher}>(({dispatch},ref)=>{
  useImperativeHandle(ref,()=>({
    restart:async()=>{frameScenario.screen='form';frameScenario.restarts+=1},
    inspect:async(_input,submit)=>{
      frameScenario.inFlight+=1;frameScenario.maxInFlight=Math.max(frameScenario.maxInFlight,frameScenario.inFlight)
      try{
      const hasForm=frameScenario.screen==='form'
      if(frameScenario.active&&hasForm&&submit){
        const task=await dispatch('invoke',{input:{}},'submit',()=>undefined) as {taskId:string}
        await new Promise(resolve=>setTimeout(resolve,250))
        await dispatch('getTask',{taskId:task.taskId},'query',()=>undefined)
        frameScenario.screen='task'
      }
      return {sourceHash:hash,errors:hasForm?[]:[{code:'PAGE_BUSINESS_FIELD_MISSING',message:'The initial form is not in the task view'}],fields:[],actions:[],visibleText:''}
      }finally{frameScenario.inFlight-=1}
    },
  }),[dispatch])
  return null
})}))
vi.mock('./page-types',async importOriginal=>({...await importOriginal<typeof import('./page-types')>(),hashPageHtml:async()=>`sha256:${'0'.repeat(64)}`}))
const draft:PageDraft={id:'draft',applicationId:'application',revision:0,html:'',htmlHash:hash,contractHash:hash,
  contract:{applicationId:'application',applicationName:'Quotation',description:null,deploymentId:'deployment',workflowVersionId:'workflow',bundleId:'bundle',
    inputSchema:{type:'object'},outputSchema:{type:'object'},waitInputNodes:[],approvalStages:[],chatMapping:null,sdkVersion:1,maxUploadBytes:1024}}

beforeEach(()=>{
  Object.assign(frameScenario,{active:false,activeRun:false,screen:'form',restarts:0,inFlight:0,maxInFlight:0})
  sessionStorage.clear();vi.mocked(apiRequest).mockReset()
  vi.mocked(apiRequest).mockImplementation(async(path,init)=>{
    if(path==='/platform-ai/model-setting')return {status:'ready',modelAlias:'glm-5.3-flash'}
    if(path.endsWith('/builder-sessions'))return {id:'session',snapshot:JSON.parse(String(init?.body)).snapshot,runs:frameScenario.activeRun?[{id:'run',status:'waiting'}]:[]}
    if(path.endsWith('/snapshot'))return {synced:true}
    if(path.endsWith('/runs'))return {id:'run',status:'queued'}
    if(path.endsWith('/checks'))return {recorded:true}
    throw new Error(`Unexpected authoring request ${path}`)
  })
})

it('adds guidance to the original active Run and retries a lost response with the same message identity',async()=>{
  frameScenario.activeRun=true
  const normal=vi.mocked(apiRequest).getMockImplementation()!
  let messages=0
  vi.mocked(apiRequest).mockImplementation(async(path,init)=>{
    if(path.endsWith('/inputs')){
      messages+=1
      if(messages===1)throw new Error('Guidance response was lost')
      return {accepted:true,runId:'run',messageId:JSON.parse(String(init?.body)).messageId,replayed:true}
    }
    return normal(path,init)
  })
  const query=new QueryClient({defaultOptions:{queries:{retry:false}}})
  const rendered=render(<QueryClientProvider client={query}><PageEditor draft={draft}/></QueryClientProvider>)
  try{
    await waitFor(()=>expect(vi.mocked(apiRequest).mock.calls.some(([path])=>path.endsWith('/snapshot'))).toBe(true))
    const panel=within(screen.getByTestId('application-page-agent'))
    const input=panel.getByRole('textbox')
    fireEvent.change(input,{target:{value:'Keep the current task and fix its waiting form.'}})
    const send=panel.getByRole('button',{name:'Add guidance'})
    expect(send).toBeEnabled();fireEvent.click(send)
    await screen.findByText('Guidance response was lost')
    await waitFor(()=>expect(send).toBeEnabled())
    fireEvent.click(send)
    await waitFor(()=>expect(input).toHaveValue(''))
    const requests=vi.mocked(apiRequest).mock.calls.filter(([path])=>path.endsWith('/inputs'))
    expect(requests).toHaveLength(2)
    expect(requests.every(([path])=>path==='/design-agent/sessions/session/runs/run/inputs')).toBe(true)
    const bodies=requests.map(([,init])=>JSON.parse(String(init?.body)))
    expect(bodies[0]).toEqual(bodies[1])
    expect(bodies[0]).toMatchObject({kind:'steering',text:'Keep the current task and fix its waiting form.'})
    expect(bodies[0].messageId).toMatch(/^[\da-f-]{36}$/)
    expect(bodies[0].editorInstanceId).toMatch(/^[\da-f-]{36}$/)
    expect(vi.mocked(apiRequest).mock.calls.filter(([path])=>path.endsWith('/runs'))).toHaveLength(0)
  }finally{rendered.unmount();query.clear()}
})

it('opens without an automatic model Run and keeps HTML read-only',async()=>{
  const query=new QueryClient({defaultOptions:{queries:{retry:false}}})
  const rendered=render(<QueryClientProvider client={query}><PageEditor draft={draft}/></QueryClientProvider>)
  await waitFor(()=>expect(vi.mocked(apiRequest).mock.calls.some(([path])=>path.endsWith('/snapshot'))).toBe(true))
  expect(vi.mocked(apiRequest).mock.calls.filter(([path])=>path.endsWith('/runs'))).toHaveLength(0)
  fireEvent.mouseDown(screen.getByRole('tab',{name:'HTML source'}),{button:0,ctrlKey:false})
  expect(await screen.findByTestId('application-page-source')).toHaveAttribute('readonly')
  expect(screen.queryByRole('button',{name:'Save page'})).not.toBeInTheDocument()
  rendered.unmount();query.clear()
})

it('binds only once when StrictMode replays the mounting effect',async()=>{
  const query=new QueryClient({defaultOptions:{queries:{retry:false}}})
  const rendered=render(<StrictMode><QueryClientProvider client={query}><PageEditor draft={draft}/></QueryClientProvider></StrictMode>)
  try{
    await waitFor(()=>expect(vi.mocked(apiRequest).mock.calls.some(([path])=>path.endsWith('/snapshot'))).toBe(true))
    expect(vi.mocked(apiRequest).mock.calls.filter(([path])=>path.endsWith('/builder-sessions'))).toHaveLength(1)
    expect(vi.mocked(apiRequest).mock.calls.filter(([path])=>path.endsWith('/runs'))).toHaveLength(0)
  }finally{rendered.unmount();query.clear()}
})

it('rechecks a page that switches to a task view using a fresh isolated form each time',async()=>{
  frameScenario.active=true
  const query=new QueryClient({defaultOptions:{queries:{retry:false}}})
  const rendered=render(<QueryClientProvider client={query}><PageEditor draft={draft}/></QueryClientProvider>)
  const reports=()=>vi.mocked(apiRequest).mock.calls.filter(([path])=>path.endsWith('/checks')).map(([,init])=>JSON.parse(String(init?.body)))
  try{
    await waitFor(()=>expect(vi.mocked(apiRequest).mock.calls.some(([path])=>path.endsWith('/snapshot'))).toBe(true))
    fireEvent.mouseDown(screen.getByRole('tab',{name:'Check results'}),{button:0,ctrlKey:false})
    const check=await screen.findByRole('button',{name:'Check again'})
    fireEvent.click(check)
    await waitFor(()=>expect(reports()).toHaveLength(2),{timeout:5000})
    expect(reports().every(report=>report.errors.length===0)).toBe(true)
    fireEvent.click(check)
    await waitFor(()=>expect(reports()).toHaveLength(4),{timeout:10000})
    expect(reports().every(report=>report.errors.length===0)).toBe(true)
    fireEvent.click(check);fireEvent.click(check)
    await waitFor(()=>expect(reports()).toHaveLength(8),{timeout:10000})
    expect(reports().every(report=>report.errors.length===0)).toBe(true)
    expect(frameScenario.maxInFlight).toBe(1)
    expect(frameScenario.restarts).toBe(8)
    expect(reports().every(report=>report.formalRequests===0&&report.synthetic)).toBe(true)
  }finally{rendered.unmount();query.clear()}
})
