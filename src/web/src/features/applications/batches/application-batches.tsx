import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { apiRequest } from '../../../shared/api/client'
import type { components } from '../../../shared/api/generated-gateway'
import { Button } from '../../../shared/ui/button'
import { Select } from '../../../shared/ui/select'
import { Table,TableCell,TableContainer,TableHead } from '../../../shared/ui/table'
import { StatusBadge } from '../../../shared/components/status-badge'
import { useLocaleFormat } from '../../../shared/lib/locale-format'
import { BatchDetail } from './batch-detail'

export function BatchState({state}:{state:string}){const {t}=useTranslation();return <StatusBadge label={t(`applicationBatches.${state}`,{defaultValue:state})} status={state==='completed'?'completed':['failed','partial_failure','outcome_unknown'].includes(state)?'failed':state==='cancelled'?'inactive':'running'}/>}

export function ApplicationBatches({applicationId}:{applicationId:string}){
  const {t}=useTranslation(),{formatDateTime}=useLocaleFormat(),[params,setParams]=useSearchParams()
  const [source,setSource]=useState('all'),[state,setState]=useState('all'),[cursors,setCursors]=useState<Array<string|undefined>>([undefined])
  const batchId=params.get('batchId'),cursor=cursors.at(-1)
  const query=new URLSearchParams({limit:'20',...(source!=='all'?{source}:{}),...(state!=='all'?{state}:{}),...(cursor?{cursor}:{})})
  const records=useQuery({queryKey:['application-batches',applicationId,query.toString()],enabled:!batchId,queryFn:()=>apiRequest<components['schemas']['BatchesPageV1']>(`/applications/${applicationId}/batches?${query}`),refetchInterval:5000})
  const open=(id?:string)=>setParams(current=>{const p=new URLSearchParams(current);p.set('recordTab','batch');if(id)p.set('batchId',id);else p.delete('batchId');return p})
  if(batchId)return <BatchDetail applicationId={applicationId} batchId={batchId} open={open}/>
  return <section className="rounded-lg border border-border bg-surface" data-testid="application-batches">
    <div className="flex gap-3 border-b border-border p-4"><Select aria-label={t('applicationBatches.source')} value={source} onValueChange={value=>{setSource(value);setCursors([undefined])}} options={['all','web','api'].map(value=>({value,label:t(`applicationBatches.${value}`)}))}/><Select aria-label={t('applicationBatches.status')} value={state} onValueChange={value=>{setState(value);setCursors([undefined])}} options={['all','running','waiting','completed','partial_failure','cancelled','outcome_unknown'].map(value=>({value,label:t(`applicationBatches.${value}`)}))}/></div>
    {records.error?<p role="alert" className="p-4 text-sm text-danger">{records.error.message}</p>:records.isPending?<p className="p-4 text-sm text-muted-foreground">{t('common.loading')}</p>:<TableContainer><Table><thead><tr>{['name','source','version','progress','status','created'].map(key=><TableHead key={key}>{t(`applicationBatches.${key}`)}</TableHead>)}<TableHead/></tr></thead><tbody>{records.data?.items.map(batch=><tr key={batch.batchId} className="border-t border-border"><TableCell><strong>{batch.name}</strong><p className="font-mono text-xs text-muted-foreground">{batch.batchId}</p><p className="max-w-60 truncate text-xs text-muted-foreground" title={batch.callerId}>{batch.callerName??(batch.callerType==='user'?t('applications.unknownUser'):batch.callerId)}</p></TableCell><TableCell>{t(`applicationBatches.${batch.source}`)}</TableCell><TableCell className="font-mono text-xs">v{batch.workflowVersionNumber}</TableCell><TableCell>{batch.counts.ended} / {batch.counts.total}</TableCell><TableCell><BatchState state={batch.state}/></TableCell><TableCell>{formatDateTime(batch.createdAt)}</TableCell><TableCell><Button variant="ghost" size="sm" onClick={()=>open(batch.batchId)}>{t('applicationBatches.detail')}</Button></TableCell></tr>)}{!records.data?.items.length&&<tr><TableCell colSpan={7}>{t('applicationBatches.empty')}</TableCell></tr>}</tbody></Table></TableContainer>}
    <div className="flex justify-end gap-2 border-t border-border p-3"><Button variant="ghost" size="sm" disabled={cursors.length<2} onClick={()=>setCursors(values=>values.slice(0,-1))}>{t('common.previous')}</Button><Button variant="secondary" size="sm" disabled={!records.data?.next} onClick={()=>setCursors(values=>[...values,records.data?.next??undefined])}>{t('common.next')}</Button></div>
  </section>
}
