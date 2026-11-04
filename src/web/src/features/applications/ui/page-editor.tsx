import { useQuery } from '@tanstack/react-query'
import { Send, Square, Check, Code2, Eye, FileDiff } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { flushSync } from 'react-dom'
import { useTranslation } from 'react-i18next'

import { useAuth } from '../../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../../shared/api/client'
import { DesignAgentEntry, type DesignEntryPayload } from '../../../shared/components/design-agent-entry'
import { useDesignAgentEvents } from '../../../shared/components/use-design-agent-events'
import { useDesignAgentInput } from '../../../shared/components/use-design-agent-input'
import { Button } from '../../../shared/ui/button'
import { MarkdownContent } from '../../../shared/ui/markdown-content'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../../shared/ui/tabs'
import { Textarea } from '../../../shared/ui/textarea'
import { PageFrame } from './page-frame'
import { PagePublicationPanel } from './page-publication-panel'
import { PAGE_SDK_SOURCE } from './page-sdk'
import { PAGE_STYLE_SOURCE } from './page-styles'
import { createPagePreview } from './page-preview'
import { inspectPagePreview } from './page-checks'
import { hashPageHtml, type PageAck, type PageCheckReport as CheckReport, type PageCommand, type PageDraft, type PageSession, type PageSnapshot, type PageFrameHandle } from './page-types'

type Entry={sequence:number;runId:string|null;payload:DesignEntryPayload&{tool?:string;commandId?:string;command?:PageCommand&{mode?:'static'|'preview';localRevision?:number};result?:unknown}}
const active=new Set(['queued','running','waiting'])

export function PageEditor({draft}:{draft:PageDraft}) {
  const {t,i18n}=useTranslation()
  const auth=useAuth()
  const instance=useRef('')
  const frame=useRef<PageFrameHandle>(null)
  const messages=useRef<HTMLDivElement>(null)
  const followMessages=useRef(true)
  const [page,setPage]=useState<PageSnapshot>({editorInstanceId:'',localRevision:0,serverRevision:draft.revision,html:draft.html,htmlHash:draft.htmlHash,saveStatus:'saved'})
  const current=useRef(page)
  const [session,setSession]=useState<PageSession>()
  const sessionRef=useRef<PageSession>()
  const bindingRequest=useRef<{key:string;request:Promise<PageSession>}>()
  const [run,setRun]=useState<{id:string;status:string}>()
  const [entries,setEntries]=useState<Entry[]>([])
  const [prompt,setPrompt]=useState('')
  const [liveText,setLiveText]=useState('')
  const [error,setError]=useState('')
  const [busy,setBusy]=useState(false)
  const [checking,setChecking]=useState(false)
  const [view,setView]=useState('preview')
  const [report,setReport]=useState<CheckReport>()
  const [difference,setDifference]=useState<{before:string;after:string;summary:string}>()
  useEffect(()=>{if(messages.current&&followMessages.current)messages.current.scrollTop=messages.current.scrollHeight},[entries,liveText,error])
  const saves=useRef<Promise<void>>(Promise.resolve())
  const checkQueue=useRef<Promise<unknown>>(Promise.resolve())
  const receipts=useRef<Record<string,PageAck>>({})
  const mounted=useRef(true)
  const queueInput=useDesignAgentInput()
  const setting=useQuery({queryKey:['platform-ai-model-setting',auth.user?.companyId,auth.user?.id],queryFn:()=>apiRequest<{status:string;modelAlias:string|null}>('/platform-ai/model-setting'),refetchInterval:5000})
  const preview=useMemo(()=>createPagePreview(draft.contract,draft.contractHash,i18n.language,document.documentElement.classList.contains('dark')?'dark':'light'),[draft.contract,draft.contractHash,i18n.language])
  const storageKey=`agentx:page-editor:${auth.user?.companyId}:${auth.user?.id}:${draft.id}`
  const update=useCallback((snapshot:PageSnapshot)=>{current.current=snapshot;if(mounted.current)setPage(snapshot)},[])
  const snapshot=useCallback(async():Promise<PageSnapshot>=>{
    const before=current.current,hash=await hashPageHtml(before.html)
    if(current.current.localRevision!==before.localRevision||current.current.html!==before.html)return snapshot()
    const value={...current.current,htmlHash:hash,editorInstanceId:instance.current};current.current=value;return value
  },[])
  const sync=useCallback(async()=>{const value=sessionRef.current;if(value)await apiRequest(`/application-ui/sessions/${value.id}/snapshot`,{method:'PUT',body:jsonBody(await snapshot())})},[snapshot])
  const save=useCallback((saveId:string=crypto.randomUUID())=>{
    const next=saves.current.catch(()=>undefined).then(async()=>{
      const before=await snapshot()
      if(before.saveStatus==='saved'){await sync();return}
      const saved=await apiRequest<{revision:number;htmlHash:string}>(`/applications/${draft.applicationId}/ui/drafts/${draft.id}`,{method:'PUT',body:jsonBody({saveId,baseRevision:before.serverRevision,html:before.html})})
      const latest=await snapshot()
      update({...latest,serverRevision:saved.revision,saveStatus:latest.htmlHash===saved.htmlHash?'saved':'unsaved'})
      if(current.current.saveStatus==='saved')sessionStorage.removeItem(`${storageKey}:recovery`)
      await sync()
    })
    saves.current=next;return next
  },[draft.applicationId,draft.id,snapshot,storageKey,sync,update])
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;preview.close()}},[preview])
  useEffect(()=>{preview.reset()},[page.html,preview])
  useEffect(()=>{
    let alive=true
    if(!window.name.startsWith('agentx-editor-window:'))window.name=`agentx-editor-window:${crypto.randomUUID()}`
    const bindingKey=`${storageKey}:${window.name}`
    instance.current=sessionStorage.getItem(bindingKey)??crypto.randomUUID();sessionStorage.setItem(bindingKey,instance.current)
    void(async()=>{
      const value=await snapshot()
      const key=JSON.stringify([draft.id,value.editorInstanceId,value.serverRevision,value.htmlHash])
      if(bindingRequest.current?.key!==key)bindingRequest.current={key,request:apiRequest<PageSession>(`/applications/${draft.applicationId}/ui/drafts/${draft.id}/builder-sessions`,{method:'POST',body:jsonBody({snapshot:value})})}
      const bound=await bindingRequest.current.request
      if(!alive)return
      const latest=await snapshot()
      let restored={...latest,editorInstanceId:instance.current,serverRevision:Math.max(latest.serverRevision,bound.snapshot.serverRevision),localRevision:Math.max(latest.localRevision,bound.snapshot.localRevision+(latest.htmlHash===bound.snapshot.htmlHash?0:1))}
      const recovery=sessionStorage.getItem(`${storageKey}:recovery`)
      if(recovery){const candidate=JSON.parse(recovery) as PageSnapshot;if(candidate.serverRevision===draft.revision&&candidate.html!==draft.html)restored={...candidate,editorInstanceId:instance.current,htmlHash:await hashPageHtml(candidate.html),saveStatus:'unsaved'}}
      update(restored);sessionRef.current=bound;setSession(bound);setRun(bound.runs.find(item=>active.has(item.status)))
      const stored=sessionStorage.getItem(`${storageKey}:receipts`)
      if(stored){const metadata=JSON.parse(stored) as Record<string,Omit<PageAck,'snapshot'>&{snapshot:Omit<PageSnapshot,'html'>}>;for(const [id,receipt] of Object.entries(metadata))if(receipt.snapshot.htmlHash===restored.htmlHash)receipts.current[id]={...receipt,snapshot:{...receipt.snapshot,html:restored.html}}}
      await sync()
    })().catch(cause=>{if(alive)setError((cause as Error).message)})
    return()=>{alive=false}
  },[draft.applicationId,draft.id,draft.html,draft.revision,snapshot,storageKey,sync,update])
  useEffect(()=>{
    if(!session)return
    const timer=setTimeout(()=>{if(current.current.saveStatus==='unsaved')void save().catch(cause=>setError((cause as Error).message))},400)
    return()=>clearTimeout(timer)
  },[page.localRevision,page.saveStatus,save,session])
  const check=useCallback((mode:'static'|'preview'):Promise<CheckReport>=>{
    const operation=checkQueue.current.catch(()=>undefined).then(async()=>{
    await save()
    const before=await snapshot()
    if(!frame.current)throw new Error('PAGE_NOT_READY')
    flushSync(()=>{setChecking(true);setView('preview')})
    try{
    if(mode==='static'){
      const restarted=frame.current.restart()
      preview.reset()
      await restarted
      await new Promise(resolve=>setTimeout(resolve,100))
    }
    const inspected=mode==='preview'?await inspectPagePreview(frame.current,preview,{invoke:t('applicationUi.missingInvoke'),updates:t('applicationUi.missingTaskUpdates'),wait:t('applicationUi.missingWait')}):await frame.current.inspect()
    const errors=[...inspected.errors]
    const diagnosticText=inspected.visibleText
    if(inspected.sourceHash!==before.htmlHash)errors.push({code:'PAGE_CHECK_STALE',message:t('applicationUi.stale')})
    if((await snapshot()).htmlHash!==before.htmlHash)errors.push({code:'PAGE_CHECK_STALE',message:t('applicationUi.stale')})
    const result:CheckReport={htmlHash:before.htmlHash,contractHash:draft.contractHash,sdkVersion:draft.contract.sdkVersion,mode,synthetic:true,formalRequests:0,errors,fields:inspected.fields,actions:inspected.actions,diagnosticText}
    await apiRequest(`/applications/${draft.applicationId}/ui/drafts/${draft.id}/checks`,{method:'POST',body:jsonBody(result)})
    setReport(result);return result
    }finally{setChecking(false)}
    })
    checkQueue.current=operation
    return operation
  },[draft.applicationId,draft.id,draft.contract.sdkVersion,draft.contractHash,preview,save,snapshot,t])
  const remember=useCallback((ack:PageAck)=>{
    receipts.current[ack.commandId]=ack
    const metadata=Object.fromEntries(Object.entries(receipts.current).slice(-16).map(([id,value])=>{const {html:_html,...snapshot}=value.snapshot;return[id,{...value,snapshot}]}))
    sessionStorage.setItem(`${storageKey}:receipts`,JSON.stringify(metadata))
  },[storageKey])
  const process=useCallback(async(entry:Entry)=>{
    const payload=entry.payload
    if(payload.type!=='model_delta')setEntries(values=>values.some(value=>value.sequence===entry.sequence)?values:[...values,entry])
    if(payload.type==='model_delta')setLiveText(value=>value+(payload.text??''))
    if(payload.type==='message_added'&&payload.message?.role==='assistant')setLiveText('')
    if(payload.type==='terminal')setRun(value=>value?.id===entry.runId?{...value,status:payload.status??'failed'}:value)
    if(payload.type!=='page_command'||!payload.commandId||!payload.command||!sessionRef.current||!mounted.current)return
    const bound=sessionRef.current
    const receipt=await apiRequest<{status:string}>(`/design-agent/sessions/${bound.id}/commands/${payload.commandId}/receipt`)
    if(receipt.status!=='pending')return
    let ack=receipts.current[payload.commandId]
    if(!ack){
      await saves.current.catch(()=>undefined)
      const before=await snapshot(),command=payload.command
      let status:PageAck['status']='stale',report:unknown=null
      if(payload.tool==='page_edit'&&command.editorInstanceId===instance.current&&command.uiDraftId===draft.id){
        if(before.htmlHash===command.baseHtmlHash&&before.localRevision===command.baseLocalRevision&&before.serverRevision===command.baseServerRevision){
          if(await hashPageHtml(command.html)!==command.htmlHash)throw new Error('PAGE_HASH_MISMATCH')
          update({...before,html:command.html,htmlHash:command.htmlHash,localRevision:before.localRevision+1,saveStatus:'unsaved'})
          setDifference({before:before.html,after:command.html,summary:command.summary})
          try{await save(command.commandId);const saved=await snapshot();status=saved.htmlHash===command.htmlHash&&saved.localRevision===command.baseLocalRevision+1?'applied':'stale'}catch(cause){status='rejected';setError((cause as Error).message)}
        }else if(before.htmlHash===command.htmlHash&&before.saveStatus==='saved'&&before.localRevision===command.baseLocalRevision+1){status='applied'}
      }else if(payload.tool==='page_check'&&command.htmlHash===before.htmlHash){
        try{report=await check(command.mode??'static');status='applied'}catch(cause){status='rejected';report={errors:[{code:'PAGE_CHECK_FAILED',message:(cause as Error).message}]}}
      }
      ack={commandId:payload.commandId,status,snapshot:await snapshot(),report};remember(ack)
    }
    await apiRequest(`/application-ui/sessions/${bound.id}/acknowledgements`,{method:'POST',body:jsonBody(ack)})
  },[check,draft.id,remember,save,snapshot,update])
  useDesignAgentEvents<Entry>(session?.id,process,cause=>setError(cause.message))
  const send=useCallback(async(text=prompt)=>{
    if(!sessionRef.current||!text.trim()||busy)return false
    setBusy(true);setError('')
    try{await save();if(run&&active.has(run.status)){await queueInput(sessionRef.current.id,run.id,instance.current,text.trim());setPrompt('');return true}const id=crypto.randomUUID();const accepted=await apiRequest<{id:string;status:string}>(`/application-ui/sessions/${sessionRef.current.id}/runs`,{method:'POST',body:jsonBody({runId:id,prompt:text,snapshot:await snapshot()})});setRun(accepted);setPrompt('');return true}
    catch(cause){setError((cause as Error).message);return false}finally{setBusy(false)}
  },[busy,prompt,queueInput,run,save,snapshot])
  const prepare=async()=>{
    const staticReport=await check('static')
    const previewReport=await check('preview')
    if(staticReport.errors.length||previewReport.errors.length){setView('checks');throw new Error(t('applicationUi.checkFailed'))}
    return snapshot()
  }
  const stop=async()=>{if(session&&run){await apiRequest(`/design-agent/sessions/${session.id}/runs/${run.id}/cancel`,{method:'POST'});setRun({...run,status:'cancelled'})}}
  const answer=async(id:string,text:string)=>{if(session)await apiRequest(`/design-agent/sessions/${session.id}/answers`,{method:'POST',body:jsonBody({questionId:id,editorInstanceId:instance.current,answer:text})})}
  const visible=entries.filter(entry=>['user','answer','question','progress','model_failed','usage','terminal','page_acknowledged'].includes(entry.payload.type)||(entry.payload.type==='message_added'&&['assistant','tool_result'].includes(entry.payload.message?.role??'')&&entry.payload.message?.content))
  return <div className="flex h-[calc(100dvh-320px)] min-h-[400px] flex-col overflow-hidden rounded-lg border border-border bg-surface" data-testid="application-page-editor"><header className="flex min-h-16 shrink-0 items-center justify-between gap-4 border-b border-border px-4"><div><h2 className="text-sm font-semibold">{t('applicationUi.workspace')}</h2><p className="mt-1 text-xs text-muted-foreground">{t('applicationUi.workspaceHint')}</p></div><PagePublicationPanel draft={draft} snapshot={page} working={Boolean(run&&active.has(run.status))} onPrepare={prepare}/></header><div className="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_400px]">
    <div className="min-h-0 min-w-0 border-r border-border">
      <Tabs className="flex h-full min-h-0 flex-col" value={view} onValueChange={setView}>
        <div className="flex items-center justify-between gap-3 border-b border-border px-3"><TabsList><TabsTrigger value="preview" disabled={checking}><Eye className="mr-1.5 size-3.5"/>{t('applicationUi.preview')}</TabsTrigger><TabsTrigger value="code" disabled={checking}><Code2 className="mr-1.5 size-3.5"/>{t('applicationUi.code')}</TabsTrigger><TabsTrigger value="diff" disabled={checking}><FileDiff className="mr-1.5 size-3.5"/>{t('applicationUi.diff')}</TabsTrigger><TabsTrigger value="checks" disabled={checking}><Check className="mr-1.5 size-3.5"/>{t('applicationUi.checks')}</TabsTrigger></TabsList><span className="text-xs text-muted-foreground">{t(page.saveStatus==='saved'?'applicationUi.saved':'applicationUi.unsaved')}</span></div>
        <TabsContent value="preview" forceMount className={view==='preview'?'flex min-h-0 flex-1 flex-col':'hidden'}><p className="shrink-0 border-b border-border bg-muted/40 px-4 py-2 text-xs text-muted-foreground">{t('applicationUi.previewHint')}</p><div className="min-h-0 flex-1"><PageFrame fit ref={frame} html={page.html} sdkScript={PAGE_SDK_SOURCE} stylesheet={PAGE_STYLE_SOURCE} identity={`${auth.user?.id}:${draft.id}`} dispatch={preview.dispatch} onDisconnect={preview.close}/></div></TabsContent>
        <TabsContent value="code" className="flex min-h-0 flex-1 flex-col gap-3 p-4"><p className="text-xs text-muted-foreground">{t('applicationUi.sourceHint')}</p><Textarea readOnly aria-label={t('applicationUi.source')} className="min-h-0 flex-1 resize-none font-mono text-xs" data-testid="application-page-source" value={page.html}/></TabsContent>
        <TabsContent value="diff" className="min-h-0 flex-1 space-y-3 overflow-auto p-4">{difference?<><p className="text-sm">{difference.summary}</p><div className="grid grid-cols-2 gap-3">{(['before','after'] as const).map(side=><div className="min-w-0" key={side}><p className="mb-2 text-xs text-muted-foreground">{t(`applicationUi.${side}`)}</p><pre className="max-h-[600px] overflow-auto whitespace-pre-wrap rounded border border-border p-3 text-[11px]">{difference[side]}</pre></div>)}</div></>:<p className="text-sm text-muted-foreground">{t('applicationUi.noDiff')}</p>}</TabsContent>
        <TabsContent value="checks" className="min-h-0 flex-1 space-y-3 overflow-auto p-4" data-testid="application-page-checks"><Button size="sm" variant="secondary" onClick={()=>void check('static').then(()=>check('preview')).then(()=>setView('checks')).catch(cause=>setError((cause as Error).message))}>{t('applicationUi.check')}</Button>{report?<><p className={report.errors.length?'text-sm text-danger':'text-sm text-success'}>{t(report.htmlHash!==page.htmlHash?'applicationUi.checkStale':report.errors.length?'applicationUi.checkFailed':'applicationUi.checkPassed')}</p>{report.errors.map((issue,index)=><p className="rounded border border-border p-3 text-xs" key={index}>{issue.code} · {issue.message}</p>)}</>:<p className="text-sm text-muted-foreground">{t('applicationUi.noChecks')}</p>}</TabsContent>
      </Tabs>
    </div>
    <aside className="flex min-h-0 flex-col" data-testid="application-page-agent"><header className="shrink-0 space-y-2 border-b border-border p-4"><h3 className="text-sm font-semibold">{t('applicationUi.conversation')}</h3><p className="text-xs text-muted-foreground">{t('applicationUi.conversationHint')}</p><p className="text-xs text-muted-foreground">{setting.data?.status==='ready'?t('applicationUi.model',{model:setting.data.modelAlias}):t('applicationUi.modelMissing')}</p></header>
      <div ref={messages} onScroll={()=>{const value=messages.current;if(value)followMessages.current=value.scrollHeight-value.scrollTop-value.clientHeight<48}} className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4 text-xs" aria-live="polite">{!entries.length&&!run&&<p className="py-8 leading-6 text-muted-foreground">{t('applicationUi.chatEmpty')}</p>}{visible.map(entry=><div key={entry.sequence}><DesignAgentEntry currentHash={page.htmlHash} payload={entry.payload} answered={entries.some(value=>value.payload.type==='answer'&&value.payload.questionId===entry.payload.questionId)} answer={answer}/>{entry.payload.type==='page_acknowledged'&&<p className="rounded border border-border p-3 text-muted-foreground">{t((entry.payload.result as {status?:string})?.status==='applied'?'applicationUi.applied':'applicationUi.rejected')}</p>}</div>)}{liveText&&<MarkdownContent>{liveText}</MarkdownContent>}{run&&active.has(run.status)&&<p className="text-muted-foreground">{t('applicationUi.working')}</p>}{error&&<p className="text-danger" role="alert">{error}</p>}</div>
      <form className="shrink-0 space-y-3 border-t border-border p-3" onSubmit={event=>{event.preventDefault();void send()}}><Textarea aria-label={t('applicationUi.prompt')} maxLength={run&&active.has(run.status)?2048:undefined} placeholder={t(run&&active.has(run.status)?'studio.builder.guidancePlaceholder':'applicationUi.promptPlaceholder')} value={prompt} onChange={event=>setPrompt(event.target.value)}/><div className="flex justify-end gap-2"><Button type="button" disabled={!run||!active.has(run.status)} size="sm" variant="secondary" onClick={()=>void stop().catch(cause=>setError((cause as Error).message))}><Square className="size-3"/>{t('applicationUi.stop')}</Button><Button disabled={!session||setting.data?.status!=='ready'||busy||!prompt.trim()} type="submit" size="sm"><Send className="size-3"/>{t(run&&active.has(run.status)?'studio.builder.guidance':'applicationUi.send')}</Button></div></form>
    </aside>
  </div></div>
}
