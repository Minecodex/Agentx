import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { connectPageBridge, type PageDispatcher } from './page-bridge'
import { createPageController } from './page-isolation'
import { pageSdkBootstrap } from './page-sdk'
import { pageStyles } from './page-styles'
import {hashPageHtml,type PageInspection,type PageFrameHandle} from './page-types'

export const PageFrame = forwardRef<PageFrameHandle,{html:string;sdkScript:string;stylesheet:string;identity:string;dispatch:PageDispatcher;onDisconnect?:()=>void;fit?:boolean}>(({html,sdkScript,stylesheet,identity,dispatch,onDisconnect,fit=false},ref)=>{
  const { t } = useTranslation()
  const iframe = useRef<HTMLIFrameElement>(null)
  const [source,setSource] = useState<{bindingId:string;document:string}>()
  const [error,setError] = useState('')
  const [generation,setGeneration] = useState(0)
  const requestedGeneration=useRef(0)
  const restartWaiter=useRef<{generation:number;resolve:()=>void;reject:(error:Error)=>void}>()
  const disconnect=useRef<()=>void>(()=>undefined)
  const inspector=useRef<PageFrameHandle['inspect']>(()=>Promise.reject(new Error('PAGE_NOT_READY')))
  useImperativeHandle(ref,()=>({inspect:(...args)=>inspector.current(...args),restart:()=>{
    // Retire the old capability before its synthetic state is reset by the host.
    disconnect.current()
    restartWaiter.current?.reject(new Error('PAGE_RELOADED'))
    const next=++requestedGeneration.current
    return new Promise<void>((resolve,reject)=>{restartWaiter.current={generation:next,resolve,reject};setGeneration(next)})
  }}),[])
  useEffect(()=>()=>{restartWaiter.current?.reject(new Error('PAGE_DISCONNECTED'));restartWaiter.current=undefined},[])
  useEffect(()=>{
    const bindingId=crypto.randomUUID().replaceAll('-','_')
    const sourceHash=hashPageHtml(html)
    const channel=new MessageChannel()
    let bound=false,closed=false
    const pending=new Map<string,{resolve:(value:PageInspection)=>void;reject:(error:Error)=>void;timer:ReturnType<typeof setTimeout>}>()
    let readyResolve:(()=>void)|undefined
    let readyReject:((error:Error)=>void)|undefined
    const ready=new Promise<void>((resolve,reject)=>{readyResolve=resolve;readyReject=reject})
    void ready.catch(()=>undefined)
    const bridge=connectPageBridge(channel.port1,(method,args,id,notify)=>dispatch(method,args,`${bindingId}:${id}`,notify))
    const close=(notify=true)=>{
      if(closed)return
      closed=true;bridge.close();channel.port2.close();readyReject?.(new Error('PAGE_DISCONNECTED'))
      if(restartWaiter.current?.generation===generation){restartWaiter.current.reject(new Error('PAGE_DISCONNECTED'));restartWaiter.current=undefined}
      for(const item of pending.values()){clearTimeout(item.timer);item.reject(new Error('PAGE_DISCONNECTED'))}pending.clear();if(notify)onDisconnect?.()
    }
    disconnect.current=()=>close(false)
    const timer=setTimeout(()=>{if(!bound){setError('PAGE_HANDSHAKE_TIMEOUT');close()}},10_000)
    const receive=(event:MessageEvent)=>{
      if(event.source!==iframe.current?.contentWindow||event.data?.bindingId!==bindingId)return
      if(event.data.type==='agentx.ui.revoked'){setError('PAGE_DISCONNECTED');close();return}
      if(event.data.type==='agentx.ui.ready'&&!bound&&!closed){
        bound=true;clearTimeout(timer);iframe.current.contentWindow?.postMessage({type:'agentx.ui.bind',bindingId},'*',[channel.port2]);readyResolve?.()
        if(restartWaiter.current?.generation===generation){restartWaiter.current.resolve();restartWaiter.current=undefined}
      }
    }
    channel.port1.onmessage=event=>{
      if(event.data?.type!=='inspection'||typeof event.data.id!=='string')return
      const item=pending.get(event.data.id)
      if(!item)return
      pending.delete(event.data.id);clearTimeout(item.timer)
      const report=event.data.report
      if(!report||!Array.isArray(report.errors)||!Array.isArray(report.fields)||!Array.isArray(report.actions)||typeof report.visibleText!=='string'||report.visibleText.length>8000){item.reject(new Error('PAGE_INSPECTION_INVALID'));return}
      void sourceHash.then(hash=>{if(closed)item.reject(new Error('PAGE_DISCONNECTED'));else item.resolve({...report,sourceHash:hash} as PageInspection)})
    }
    inspector.current=async(input,submit,waitId,options)=>{
      await ready
      if(closed)throw new Error('PAGE_DISCONNECTED')
      return new Promise<PageInspection>((resolve,reject)=>{
        const id=crypto.randomUUID()
        const timer=setTimeout(()=>{pending.delete(id);reject(new Error('PAGE_INSPECTION_TIMEOUT'))},15_000)
        pending.set(id,{resolve,reject,timer});channel.port1.postMessage({type:'inspect',id,input,submit,waitId,options})
      })
    }
    addEventListener('message',receive)
    setError('');setSource({bindingId,document:createPageController(html,bindingId,pageSdkBootstrap(bindingId,sdkScript),pageStyles(stylesheet))})
    return ()=>{clearTimeout(timer);removeEventListener('message',receive);close(false)}
  },[html,sdkScript,stylesheet,identity,dispatch,onDisconnect,generation])
  return <div className={`relative h-full bg-background ${fit?'min-h-0':'min-h-[600px]'}`} data-testid="application-page-frame">
    {error&&<div className="absolute inset-x-0 top-0 z-10 border-b border-danger/30 bg-surface p-3 text-sm text-danger" role="alert">{t('applicationUi.frameError',{code:error})}</div>}
    {source&&<iframe key={source.bindingId} className={`h-full w-full border-0 ${fit?'min-h-0':'min-h-[600px]'}`} ref={iframe} referrerPolicy="no-referrer" sandbox="allow-scripts allow-forms" srcDoc={source.document} title={t('applicationUi.previewTitle')} />}
  </div>
})
PageFrame.displayName='PageFrame'
