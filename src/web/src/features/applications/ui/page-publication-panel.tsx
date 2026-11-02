import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ExternalLink, History, Loader2, Upload, X } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest, jsonBody } from '../../../shared/api/client'
import type { components } from '../../../shared/api/generated'
import { Button } from '../../../shared/ui/button'
import { Dialog, DialogContent } from '../../../shared/ui/dialog'
import { Input } from '../../../shared/ui/input'
import { PageHistoryPreview } from './page-history-preview'
import type { PageDraft, PagePublications, PageSnapshot } from './page-types'

type Publication=PagePublications['publications'][number]
const pendingStates=new Set(['copying','preparing','activating'])

export function PagePublicationPanel({draft,snapshot,working,onPrepare}:{draft:PageDraft;snapshot:PageSnapshot;working:boolean;onPrepare:()=>Promise<PageSnapshot>}){
  const {t}=useTranslation(),cache=useQueryClient()
  const key=['application-page-publications',draft.applicationId]
  const data=useQuery({queryKey:key,queryFn:()=>apiRequest<PagePublications>(`/applications/${draft.applicationId}/ui/publications`),refetchInterval:query=>query.state.data?.publications.some(value=>pendingStates.has(value.status))?750:5000})
  const [addressOpen,setAddressOpen]=useState(false),[historyOpen,setHistoryOpen]=useState(false)
  const [slug,setSlug]=useState(''),[selected,setSelected]=useState<Publication>(),[confirm,setConfirm]=useState<Publication>()
  const current=data.data?.publications.find(value=>value.id===data.data?.activePublicationId)
  const suffix=current?.slug??data.data?.slug
  const pending=data.data?.publications.find(value=>pendingStates.has(value.status))
  const history=data.data?.publications.filter(value=>['active','superseded'].includes(value.status))??[]
  const publish=useMutation({mutationFn:async({target,record}:{target:string;record?:Publication})=>{
    const page=record?undefined:await onPrepare()
    const body:components['schemas']['PublishPage']={draftId:record?null:draft.id,uiVersionId:record?.uiVersionId??null,expectedDraftRevision:page?.serverRevision??null,expectedPublicationRevision:data.data?.revision??0,slug:target,enabled:true}
    return apiRequest<Publication>(`/applications/${draft.applicationId}/ui/publications`,{method:'POST',body:jsonBody(body)})
  },onSuccess:async()=>{setAddressOpen(false);setConfirm(undefined);await cache.invalidateQueries({queryKey:key})}})
  const locked=Boolean(pending)||publish.isPending
  const error=publish.error??data.error
  const publishCurrent=()=>{publish.reset();if(suffix)publish.mutate({target:suffix});else setAddressOpen(true)}
  return <div className="flex items-center gap-2" data-testid="application-page-publication">
    {pending&&<span role="status" className="mr-1 inline-flex items-center gap-1.5 text-xs text-muted-foreground"><Loader2 className="size-3.5 animate-spin"/>{t(`applicationUi.publishStatus.${pending.status}`)}</span>}
    {!pending&&data.data?.publications[0]?.status==='rejected'&&<span role="alert" className="text-xs text-danger">{t('applicationUi.publishStatus.rejected')} · {data.data.publications[0].error}</span>}
    {error&&!addressOpen&&<span role="alert" className="max-w-64 text-xs text-danger">{error.message}</span>}
    {current?.manifest?.enabled&&<Button asChild size="sm" variant="ghost"><a href={`/a/${current.slug}`} target="_blank" rel="noopener noreferrer" data-testid="application-open-page"><ExternalLink className="size-3.5"/>{t('applicationUi.openPage')}</a></Button>}
    <Button size="sm" variant="secondary" disabled={!history.length} onClick={()=>{setSelected(current??history[0]);setHistoryOpen(true)}} data-testid="application-page-history"><History className="size-3.5"/>{t('applicationUi.history')}</Button>
    <Button size="sm" disabled={!data.data||working||locked||snapshot.saveStatus!=='saved'||!snapshot.html.trim()} onClick={publishCurrent} data-testid="application-publish-page">{publish.isPending?<Loader2 className="size-3.5 animate-spin"/>:<Upload className="size-3.5"/>}{t(publish.isPending?'applicationUi.publishing':'applicationUi.publish')}</Button>
    <Dialog open={addressOpen} onOpenChange={value=>{if(!locked)setAddressOpen(value)}}>
      <DialogContent title={t('applicationUi.publishAddress')} description={t('applicationUi.addressHint')} className="p-6">
        <h2 className="text-lg font-semibold">{t('applicationUi.publishAddress')}</h2><p className="mt-2 text-sm text-muted-foreground">{t('applicationUi.addressHint')}</p>
        <form className="mt-5 space-y-4" onSubmit={event=>{event.preventDefault();publish.mutate({target:slug.trim()})}}>
          <label className="block space-y-2"><span className="text-xs font-medium">{t('applicationUi.slug')}</span><div className="flex items-center gap-2"><span className="shrink-0 text-sm text-muted-foreground">{window.location.origin}/a/</span><Input autoFocus aria-label={t('applicationUi.slug')} value={slug} onChange={event=>setSlug(event.target.value)} placeholder="expense-demo" pattern="[a-z](?:[a-z0-9]|-(?!-))*[a-z0-9]" minLength={3} maxLength={80} required disabled={locked}/></div></label>
          {error&&<p role="alert" className="text-sm text-danger">{error.message}</p>}
          <div className="flex justify-end gap-2"><Button type="button" variant="ghost" disabled={locked} onClick={()=>setAddressOpen(false)}>{t('common.cancel')}</Button><Button type="submit" disabled={locked||!slug.trim()}>{t(publish.isPending?'applicationUi.publishing':'applicationUi.publish')}</Button></div>
        </form>
      </DialogContent>
    </Dialog>
    <Dialog open={historyOpen} onOpenChange={setHistoryOpen}>
      <DialogContent title={t('applicationUi.history')} description={t('applicationUi.historyHint')} className="w-[min(1320px,calc(100vw-64px))]">
        <header className="flex items-center justify-between border-b border-border p-5"><div><h2 className="text-base font-semibold">{t('applicationUi.history')}</h2><p className="mt-1 text-xs text-muted-foreground">{t('applicationUi.historyHint')}</p></div><Button aria-label={t('common.close')} size="sm" variant="ghost" onClick={()=>setHistoryOpen(false)}><X className="size-4"/></Button></header>
        <div className="grid h-[min(720px,75vh)] grid-cols-[240px_minmax(0,1fr)]">
          <nav aria-label={t('applicationUi.history')} className="space-y-2 overflow-y-auto border-r border-border bg-muted/20 p-3">{history.map(record=><button key={record.id} type="button" onClick={()=>{setSelected(record);setConfirm(undefined)}} aria-pressed={record.id===selected?.id} className={`w-full rounded-md border p-3 text-left text-xs ${record.id===selected?.id?'border-primary bg-primary/5':'border-transparent hover:bg-muted'}`} data-testid={`page-history-${record.id}`}><span className="font-medium">{t('applicationUi.historyRecord',{number:record.revision})}</span>{record.id===current?.id&&<span className="ml-2 text-success">{t('applicationUi.currentlyLive')}</span>}<span className="mt-2 block truncate text-muted-foreground">/a/{record.slug}</span></button>)}</nav>
          <div className="flex min-h-0 min-w-0 flex-col"><div className="flex items-center justify-between border-b border-border px-4 py-3"><span className="text-xs text-muted-foreground">{t('applicationUi.historyPreview')}</span><Button size="sm" variant="secondary" disabled={!selected||selected.id===current?.id||locked} onClick={()=>{publish.reset();setConfirm(selected)}}>{t('applicationUi.rollback')}</Button></div>
            {confirm?<div className="border-b border-border bg-muted/20 p-4"><p className="text-sm">{t('applicationUi.rollbackHint')}</p><div className="mt-3 flex gap-2"><Button size="sm" disabled={locked} onClick={()=>publish.mutate({target:suffix??confirm.slug,record:confirm})}>{t('applicationUi.confirmRollback')}</Button><Button size="sm" variant="ghost" disabled={locked} onClick={()=>setConfirm(undefined)}>{t('common.cancel')}</Button></div>{error&&<p className="mt-2 text-xs text-danger" role="alert">{error.message}</p>}</div>:null}
            <div className="min-h-0 flex-1 overflow-auto">{selected&&<PageHistoryPreview key={selected.id} applicationId={draft.applicationId} publicationId={selected.id}/>}</div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  </div>
}
