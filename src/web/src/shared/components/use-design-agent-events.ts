import { useEffect, useRef } from 'react'

import { apiRequestStream } from '../api/client'
import { consumeServerSentEvents } from '../lib/server-sent-events'

export function useDesignAgentEvents<T extends {sequence:number}>(sessionId:string|undefined,process:(entry:T)=>Promise<void>,onError:(error:Error)=>void) {
  const latest=useRef({process,onError});latest.current={process,onError}
  useEffect(()=>{
    if(!sessionId)return
    let alive=true,cursor=0
    const controller=new AbortController()
    void(async()=>{
      while(alive){
        try{
          const response=await apiRequestStream(`/design-agent/sessions/${sessionId}/events?after=${cursor}`,{signal:controller.signal})
          await consumeServerSentEvents(response,async data=>{
            if(!alive)return
            const entry=JSON.parse(data) as T
            if(!Number.isSafeInteger(entry.sequence)||entry.sequence<=cursor)return
            await latest.current.process(entry);cursor=entry.sequence
          })
        }catch(error){if(alive)latest.current.onError(error instanceof Error?error:new Error(String(error)))}
        if(alive)await new Promise(resolve=>setTimeout(resolve,1000))
      }
    })()
    return()=>{alive=false;controller.abort()}
  },[sessionId])
}
