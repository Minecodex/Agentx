import { useQuery } from '@tanstack/react-query'
import { LogOut, Plus, RotateCw } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useParams, useSearchParams } from 'react-router-dom'

import { useAuth } from '../../../app/providers/auth-provider'
import { applicationPageClient } from '../../../shared/api/client'
import { Button } from '../../../shared/ui/button'
import { PageFrame } from './page-frame'
import { createLivePage } from './page-live'
import type { PublishedPage } from './page-types'

export function ApplicationBusinessPage(){
  const {slug=''}=useParams(),[params]=useSearchParams(),navigate=useNavigate()
  const auth=useAuth(),{t}=useTranslation()
  const taskId=params.get('taskId')
  const [epoch,setEpoch]=useState(0)
  const page=useQuery({queryKey:['application-page',auth.sessionId,slug,taskId,epoch],queryFn:()=>applicationPageClient(auth.sessionId).request<PublishedPage>(`/pages/${encodeURIComponent(slug)}${taskId?`?taskId=${encodeURIComponent(taskId)}`:''}`),retry:false})
  const restart=()=>{navigate(`/a/${encodeURIComponent(slug)}`,{replace:true});setEpoch(value=>value+1)}
  return <main className="flex h-screen min-w-[1024px] flex-col bg-background text-foreground" data-testid="application-business-shell">
    <header className="flex h-16 shrink-0 items-center justify-between gap-4 border-b border-border bg-surface px-6"><div className="min-w-0"><h1 className="truncate text-sm font-semibold">{page.data?.publication.contract.applicationName??t('applicationUi.businessTitle')}</h1><p className="text-xs text-muted-foreground">{page.data?t('applicationUi.publishedVersion',{revision:page.data.publication.revision}):'Agentx'}</p></div><div className="flex items-center gap-3"><span className="text-xs text-muted-foreground">{auth.user?.displayName}</span><Button variant="secondary" size="sm" onClick={restart} data-testid="application-new-task"><Plus className="size-3.5"/>{t('applicationUi.newTask')}</Button><Button variant="ghost" size="sm" onClick={()=>void auth.logout()}><LogOut className="size-3.5"/>{t('applicationUi.signOut')}</Button></div></header>
    {page.isPending?<div className="grid flex-1 place-items-center text-sm text-muted-foreground">{t('applicationUi.loading')}</div>:page.error?<div className="m-auto max-w-lg space-y-4 p-8"><p role="alert" className="text-sm text-danger">{page.error.message}</p><Button variant="secondary" onClick={()=>void page.refetch()}><RotateCw className="size-3.5"/>{t('applicationUi.reload')}</Button></div>:page.data?<BusinessPageHost key={`${page.data.publication.publicationId}:${taskId??''}:${auth.sessionId}:${epoch}`} view={page.data}/>:null}
  </main>
}

function BusinessPageHost({view}:{view:PublishedPage}){
  const auth=useAuth(),{t,i18n}=useTranslation(),navigate=useNavigate()
  const [error,setError]=useState('')
  const [task,setTask]=useState(view.taskId)
  const created=useCallback((id:string)=>{
    setTask(id)
    const url=new URL(window.location.href);url.searchParams.set('taskId',id);window.history.replaceState(window.history.state,'',url)
  },[])
  const failed=useCallback((error:Error)=>setError(error.message),[])
  const theme=document.documentElement.classList.contains('dark')?'dark':'light'
  const bindingKey=`${auth.sessionId}:${view.publication.publicationId}:${i18n.language}:${theme}`
  const [binding,setBinding]=useState<{key:string;live:ReturnType<typeof createLivePage>}>()
  useEffect(()=>{
    const live=createLivePage(view,auth.sessionId,i18n.language,theme,auth.user?.displayName??'',navigate,created,failed)
    setBinding({key:bindingKey,live});return()=>live.close()
  },[view,auth.sessionId,auth.user?.displayName,i18n.language,theme,navigate,created,failed,bindingKey])
  const live=binding?.key===bindingKey?binding.live:undefined
  return <section className="min-h-0 flex-1">
    {(view.historical||error)&&<div className="flex items-center justify-between gap-4 border-b border-border bg-muted/40 px-6 py-2 text-xs"><span className={error?'text-danger':'text-muted-foreground'} role={error?'alert':undefined}>{error||t('applicationUi.historyMode')}</span></div>}
    {task&&<span className="sr-only" data-testid="application-current-task">{task}</span>}
    {live&&<PageFrame html={view.html} sdkScript={view.sdkScript} stylesheet={view.stylesheet} identity={`${auth.sessionId}:${view.publication.applicationId}:${view.publication.uiVersionId}`} dispatch={live.dispatch} onDisconnect={live.close}/>}
  </section>
}
