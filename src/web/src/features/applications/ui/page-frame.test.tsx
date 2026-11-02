import { act, render, waitFor } from '@testing-library/react'
import { createRef } from 'react'
import { expect, it, vi } from 'vitest'

import { PageFrame } from './page-frame'
import type { PageFrameHandle } from './page-types'

vi.mock('./page-isolation',()=>({createPageController:(_html:string,binding:string)=>binding}))
vi.mock('./page-types',()=>({hashPageHtml:async()=> 'source-hash'}))

it('mounts the sandbox with its controller document already populated',()=>{
  const documents:string[]=[]
  const original=Element.prototype.setAttribute
  const attribute=vi.spyOn(Element.prototype,'setAttribute').mockImplementation(function(this:Element,name,value){
    if(this instanceof HTMLIFrameElement&&name.toLowerCase()==='srcdoc')documents.push(String(value))
    return original.call(this,name,value)
  })
  const rendered=render(<PageFrame html="published" sdkScript="" stylesheet="" identity="live" dispatch={async()=>null}/>)
  try{
    expect(documents.length).toBeGreaterThan(0)
    expect(documents.every(document=>document.trim().length>0)).toBe(true)
  }finally{rendered.unmount();attribute.mockRestore()}
})

it('retires the old port and inspections before a restart, and accepts only the new handshake',async()=>{
  const ref=createRef<PageFrameHandle>()
  const rendered=render(<PageFrame ref={ref} html="unchanged" sdkScript="" stylesheet="" identity="author:draft" dispatch={async()=>null}/>)
  let iframe=rendered.container.querySelector('iframe')!
  const bind=(binding:string)=>window.dispatchEvent(new MessageEvent('message',{source:iframe.contentWindow,data:{type:'agentx.ui.ready',bindingId:binding}}))
  try{
    await waitFor(()=>expect(iframe.srcdoc).not.toBe(''))
    const first=iframe.srcdoc
    const ports:MessagePort[]=[]
    vi.spyOn(iframe.contentWindow!,'postMessage').mockImplementation((...args:unknown[])=>{ports.push((args[2] as MessagePort[])[0]!)})
    act(()=>{bind(first)})
    const previousPort=ports.at(-1)!
    let inspected!:()=>void
    const requested=new Promise<void>(resolve=>{inspected=resolve})
    previousPort.onmessage=event=>{if(event.data.type==='inspect')inspected()}
    const original=ref.current!.inspect({},true)
    const rejected=expect(original).rejects.toThrow('PAGE_DISCONNECTED')
    await requested
    let restarted!:Promise<void>,ready=false
    act(()=>{restarted=ref.current!.restart();void restarted.then(()=>{ready=true})})
    await rejected
    await waitFor(()=>expect(rendered.container.querySelector('iframe')!.srcdoc).not.toBe(first))
    const retired=iframe
    iframe=rendered.container.querySelector('iframe')!
    expect(iframe).not.toBe(retired)
    vi.spyOn(iframe.contentWindow!,'postMessage').mockImplementation((...args:unknown[])=>{ports.push((args[2] as MessagePort[])[0]!)})
    const second=iframe.srcdoc
    act(()=>{bind(first)})
    await Promise.resolve()
    expect(ready).toBe(false)
    act(()=>{bind(second)})
    await restarted
    const port=ports.at(-1)!
    port.onmessage=event=>{
      if(event.data.type==='inspect')port.postMessage({type:'inspection',id:event.data.id,report:{errors:[],fields:[],actions:[],visibleText:''}})
    }
    await expect(ref.current!.inspect({},true)).resolves.toMatchObject({sourceHash:'source-hash',errors:[]})
    let abandoned!:Promise<void>
    act(()=>{abandoned=ref.current!.restart()})
    const disconnected=expect(abandoned).rejects.toThrow('PAGE_DISCONNECTED')
    rendered.unmount()
    await disconnected
  }finally{rendered.unmount()}
})
