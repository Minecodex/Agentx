import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'

import { apiRequest } from '../../../shared/api/client'
import { PagePublicationPanel } from './page-publication-panel'
import type { PageDraft, PagePublications, PageSnapshot } from './page-types'

vi.mock('../../../shared/api/client',()=>({apiRequest:vi.fn(),jsonBody:JSON.stringify}))
vi.mock('./page-history-preview',()=>({PageHistoryPreview:({publicationId}:{publicationId:string})=><div data-testid="frozen-preview">{publicationId}</div>}))
const snapshot:PageSnapshot={editorInstanceId:'window',localRevision:2,serverRevision:3,html:'<h1>Still editing this HTML</h1>',htmlHash:'hash',saveStatus:'saved'}
const draft={id:'page',applicationId:'app'} as PageDraft
let state:PagePublications
const clients:QueryClient[]=[]
beforeEach(()=>{
  state={revision:0,slug:null,enabled:false,activePublicationId:null,publications:[]}
  vi.mocked(apiRequest).mockReset().mockImplementation(async(path,init)=>{
    if(path==='/applications/app/ui/publications'&&!init)return state
    if(path==='/applications/app/ui/publications'&&init?.method==='POST')return {id:'accepted',status:'copying'}
    throw new Error('Unexpected publication request: '+path)
  })
})
afterEach(()=>{cleanup();clients.splice(0).forEach(client=>client.clear())})
function setup(prepare=vi.fn(async()=>snapshot)){
  const client=new QueryClient({defaultOptions:{queries:{retry:false}}});clients.push(client)
  render(<QueryClientProvider client={client}><PagePublicationPanel draft={draft} snapshot={snapshot} working={false} onPrepare={prepare}/></QueryClientProvider>)
  return prepare
}
const submissions=()=>vi.mocked(apiRequest).mock.calls.filter(([,init])=>init?.method==='POST').map(([,init])=>JSON.parse(String(init?.body)))
function published(){
  state={revision:2,slug:'expenses',enabled:true,activePublicationId:'live',publications:[
    {id:'live',revision:2,uiVersionId:'second-html',slug:'expenses',status:'active',manifest:null},
    {id:'old',revision:1,uiVersionId:'first-html',slug:'original-address',status:'superseded',manifest:null},
  ]}
}

it('asks for the address on first publish and checks the current HTML before submitting',async()=>{
  const prepare=setup()
  const publish=screen.getByRole('button',{name:'Publish'})
  await waitFor(()=>expect(publish).toBeEnabled());fireEvent.click(publish)
  const dialog=await screen.findByRole('dialog',{name:'Set the access address'})
  expect(submissions()).toEqual([])
  fireEvent.change(within(dialog).getByRole('textbox',{name:'Access suffix'}),{target:{value:'expenses'}})
  fireEvent.click(within(dialog).getByRole('button',{name:'Publish'}))
  await waitFor(()=>expect(submissions()).toHaveLength(1))
  expect(prepare).toHaveBeenCalledOnce()
  expect(submissions()[0]).toEqual({draftId:'page',uiVersionId:null,expectedDraftRevision:3,expectedPublicationRevision:0,slug:'expenses',enabled:true})
})

it('publishes subsequent edits directly to the existing address',async()=>{
  published();const prepare=setup()
  const publish=screen.getByRole('button',{name:'Publish'})
  await waitFor(()=>expect(publish).toBeEnabled());fireEvent.click(publish)
  await waitFor(()=>expect(submissions()).toHaveLength(1))
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  expect(prepare).toHaveBeenCalledOnce();expect(submissions()[0].slug).toBe('expenses')
})

it('previews and rolls back a publication without restoring or preparing the working HTML',async()=>{
  published();const prepare=setup()
  const history=screen.getByRole('button',{name:'Publication history'})
  await waitFor(()=>expect(history).toBeEnabled());fireEvent.click(history)
  fireEvent.click(await screen.findByTestId('page-history-old'))
  expect(screen.getByTestId('frozen-preview')).toHaveTextContent('old')
  fireEvent.click(screen.getByRole('button',{name:'Roll back to this publication'}))
  fireEvent.click(screen.getByRole('button',{name:'Confirm rollback'}))
  await waitFor(()=>expect(submissions()).toHaveLength(1))
  expect(submissions()[0]).toEqual({draftId:null,uiVersionId:'first-html',expectedDraftRevision:null,expectedPublicationRevision:2,slug:'expenses',enabled:true})
  expect(prepare).not.toHaveBeenCalled()
  expect(snapshot.html).toBe('<h1>Still editing this HTML</h1>')
  expect(vi.mocked(apiRequest).mock.calls.some(([path])=>path.includes('/restore'))).toBe(false)
})

it('retains the entered address and does not publish when the page check fails',async()=>{
  setup(vi.fn(async()=>{throw new Error('Fix the waiting form')}))
  const publish=screen.getByRole('button',{name:'Publish'})
  await waitFor(()=>expect(publish).toBeEnabled());fireEvent.click(publish)
  const dialog=await screen.findByRole('dialog',{name:'Set the access address'})
  const address=within(dialog).getByRole('textbox',{name:'Access suffix'})
  fireEvent.change(address,{target:{value:'expenses'}})
  fireEvent.click(within(dialog).getByRole('button',{name:'Publish'}))
  expect(await within(dialog).findByRole('alert')).toHaveTextContent('Fix the waiting form')
  expect(address).toHaveValue('expenses');expect(submissions()).toEqual([])
})
