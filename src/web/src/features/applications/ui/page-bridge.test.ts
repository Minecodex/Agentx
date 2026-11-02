import {expect,it,vi} from 'vitest'
import {connectPageBridge} from './page-bridge'

const taskId='01988b16-d739-7000-8000-000000000001'
// An actual deterministic Artifact ID observed in the formal file scenario.
const artifactId='26457f85-1d8a-a91b-e443-8d6ba619a5e5'

it('passes Runtime UUID-shaped Artifact identities through the private bridge for server authorization',async()=>{
  const channel=new MessageChannel(),dispatch=vi.fn(async()=>null)
  const bridge=connectPageBridge(channel.port1,dispatch)
  try{
    const response=new Promise(resolve=>{channel.port2.onmessage=event=>resolve(event.data);channel.port2.start()})
    channel.port2.postMessage({type:'request',id:'download',method:'downloadFile',args:{taskId,artifactId}})
    expect(await response).toEqual({type:'response',id:'download',result:null})
    expect(dispatch).toHaveBeenCalledWith('downloadFile',{taskId,artifactId},'download',expect.any(Function))
  }finally{bridge.close();channel.port2.close()}
})

it('still refuses malformed identities and capabilities outside the fixed SDK',async()=>{
  const channel=new MessageChannel(),dispatch=vi.fn(async()=>null)
  const bridge=connectPageBridge(channel.port1,dispatch)
  try{
    for(const [method,args]of [['downloadFile',{taskId,artifactId:'../../another-file'}],['approve',{approvalId:artifactId}]] as const){
      const response=new Promise(resolve=>{channel.port2.onmessage=event=>resolve(event.data);channel.port2.start()})
      channel.port2.postMessage({type:'request',id:method,method,args})
      expect(await response).toMatchObject({id:method,error:{code:'PAGE_REQUEST_INVALID'}})
    }
    expect(dispatch).not.toHaveBeenCalled()
  }finally{bridge.close();channel.port2.close()}
})
