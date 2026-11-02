import { useCallback, useRef } from 'react'

import { apiRequest, jsonBody } from '../api/client'
import type { components } from '../api/generated'

type Request = components['schemas']['CreateDesignInput']
type Receipt = components['schemas']['DesignInputReceipt']

/** A lost response retries the same original-Run input, including its identity. */
export function useDesignAgentInput() {
  const pending=useRef<{key:string;request:Request}>()
  return useCallback(async(session:string,run:string,editor:string,text:string)=>{
    const key=JSON.stringify([session,run,editor,text])
    if(pending.current?.key!==key)pending.current={key,request:{messageId:crypto.randomUUID(),editorInstanceId:editor,kind:'steering',text}}
    const request=pending.current.request
    const receipt=await apiRequest<Receipt>(`/design-agent/sessions/${session}/runs/${run}/inputs`,{method:'POST',body:jsonBody(request)})
    if(pending.current?.request.messageId===request.messageId)pending.current=undefined
    return receipt
  },[])
}
