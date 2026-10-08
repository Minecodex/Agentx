import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Activity, FileUp, Trash2 } from 'lucide-react'
import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useParams } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest } from '../../shared/api/client'
import type { ExternalConnection, HealthCheck, Knowledge, KnowledgeDocument, RetrievalTestResult } from '../../shared/api/types'
import { ConfirmDialog } from '../../shared/components/confirm-dialog'
import { ResourceDetailLayout } from '../../shared/components/resource-detail-layout'
import { localizedValue } from '../../shared/lib/localized-value'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { Input } from '../../shared/ui/input'
import { StatusBadge } from '../../shared/components/status-badge'
import { useToast } from '../../shared/ui/toast'

type RetrievalChunk = { documentId?: unknown, chunkId?: unknown, content?: unknown, score?: unknown }

export function KnowledgeDetailPage() {
  const { id = '' } = useParams(); const { t } = useTranslation(); const auth = useAuth(); const { showToast } = useToast(); const queryClient = useQueryClient(); const [health, setHealth] = useState<HealthCheck>(); const { formatNumber } = useLocaleFormat()
  const fileInput = useRef<HTMLInputElement>(null)
  const [removeId, setRemoveId] = useState<string>()
  const [query, setQuery] = useState(''); const [topK, setTopK] = useState('5'); const [hits, setHits] = useState<RetrievalChunk[] | null>(null)
  const resource = useQuery({ queryKey: ['knowledge', id], queryFn: () => apiRequest<Knowledge>(`/knowledge/resources/${id}`), refetchInterval: (page) => page.state.data?.syncStatus === 'syncing' ? 3_000 : false })
  const connections = useQuery({ queryKey: ['knowledge-connections'], queryFn: () => apiRequest<ExternalConnection[]>('/knowledge/connections') })
  const documents = useQuery({ queryKey: ['knowledge-documents', id], queryFn: () => apiRequest<KnowledgeDocument[]>(`/knowledge/resources/${id}/documents`), refetchInterval: (page) => (page.state.data as KnowledgeDocument[] | undefined)?.some((item) => item.status === 'uploading' || item.status === 'indexing') ? 3_000 : false })
  const test = useMutation({ mutationFn: () => apiRequest<HealthCheck>(`/knowledge/connections/${resource.data?.connectionId}/test-connection`, { method: 'POST' }), onSuccess: (value) => { setHealth(value); showToast(localizedValue(t, 'knowledge', value.status)) }, onError: (error: Error) => showToast(error.message) })
  const upload = useMutation({
    mutationFn: async (file: File) => {
      const body = new FormData(); body.append('file', file)
      return apiRequest<KnowledgeDocument>(`/knowledge/resources/${id}/documents`, { method: 'POST', body })
    },
    onSuccess: async () => { await Promise.all([queryClient.invalidateQueries({ queryKey: ['knowledge-documents', id] }), queryClient.invalidateQueries({ queryKey: ['knowledge', id] })]); showToast(t('knowledge.documentUploaded')) },
    onError: (error: Error) => showToast(error.message),
  })
  const remove = useMutation({
    mutationFn: (documentId: string) => apiRequest<void>(`/knowledge/resources/${id}/documents/${documentId}`, { method: 'DELETE' }),
    onSuccess: async () => { setRemoveId(undefined); await Promise.all([queryClient.invalidateQueries({ queryKey: ['knowledge-documents', id] }), queryClient.invalidateQueries({ queryKey: ['knowledge', id] })]); showToast(t('knowledge.documentDeleted')) },
    onError: (error: Error) => showToast(error.message),
  })
  const retrieval = useMutation({
    mutationFn: () => apiRequest<RetrievalTestResult>(`/knowledge/resources/${id}/retrieval-test`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ query: query.trim(), topK: Number(topK) || 5 }) }),
    onSuccess: (value) => setHits((value.documents ?? []) as RetrievalChunk[]),
    onError: (error: Error) => showToast(error.message),
  })
  const value = resource.data; const connection = connections.data?.find((item) => item.id === value?.connectionId)
  const externalManaged = connection?.provider === 'ragflow'
  return <ResourceDetailLayout actions={auth.hasPermission('knowledge:manage') ? <Button disabled={test.isPending} onClick={() => test.mutate()} variant="secondary"><Activity className="size-4" />{t('knowledge.connectionTest')}</Button> : undefined} description={t('knowledge.description')} details={value ? [{ label: t('knowledge.connectionName'), value: value.connectionName }, { label: t('knowledge.endpoint'), value: connection?.endpoint }, { label: t('knowledge.healthPath'), value: connection?.healthPath }, { label: t('knowledge.externalId'), value: value.externalResourceId }, { label: t('knowledge.sync'), value: localizedValue(t, 'common', value.syncStatus) }, { label: t('knowledge.department'), value: value.ownerDepartmentId }] : []} error={resource.error} loading={resource.isLoading} name={value?.name} status={value?.status}>
    {health && <Card className="p-4 text-xs text-muted-foreground">{localizedValue(t, 'knowledge', health.status)} · {health.latencyMs ?? 0} ms{health.errorMessage ? ` · ${health.errorMessage}` : ''}</Card>}
    <Card className="overflow-hidden">
      <div className="flex min-h-14 items-center justify-between border-b border-border px-5"><h2 className="text-sm font-semibold">{t('knowledge.documents')}</h2>{auth.hasPermission('knowledge:manage') && !externalManaged && <><input accept=".txt,.md,.json,.csv,text/plain,text/markdown,application/json,text/csv" className="sr-only" onChange={(event) => { const file = event.target.files?.[0]; if (file) upload.mutate(file); event.target.value = '' }} ref={fileInput} type="file" /><Button disabled={upload.isPending} onClick={() => fileInput.current?.click()} size="sm"><FileUp className="size-4" />{t('knowledge.uploadDocument')}</Button></>}</div>
      {externalManaged ? <p className="px-5 py-8 text-center text-xs text-muted-foreground">{t('knowledge.externalManaged')}</p> : <div className="px-5">
        {documents.data?.map((item) => <div className="flex items-center gap-4 border-t border-border py-3 first:border-0" key={item.id}>
          <div className="min-w-0"><p className="truncate text-xs font-medium">{item.name}</p><p className="mt-1 text-[11px] text-muted-foreground">{item.contentType} · {formatNumber(item.sizeBytes)} B{item.errorCode ? ` · ${item.errorCode}: ${item.errorMessage}` : ''}</p></div>
          <div className="flex-1" />
          <StatusBadge label={documentStatusLabel(t, item.status)} status={documentStatus(item.status)} />
          {auth.hasPermission('knowledge:manage') && <Button aria-label={t('knowledge.deleteDocument')} disabled={remove.isPending || item.status === 'indexing' || item.status === 'uploading'} onClick={() => setRemoveId(item.id)} size="icon" variant="ghost"><Trash2 className="size-4" /></Button>}
        </div>)}
        {!documents.isLoading && (documents.data?.length ?? 0) === 0 && <p className="py-8 text-center text-xs text-muted-foreground">{t('knowledge.noDocuments')}</p>}
      </div>}
    </Card>
    <Card className="overflow-hidden">
      <div className="flex min-h-14 items-center border-b border-border px-5"><h2 className="text-sm font-semibold">{t('knowledge.retrievalTest')}</h2></div>
      <div className="space-y-3 px-5 py-4">
        <div className="flex gap-2">
          <Input disabled={!auth.hasPermission('knowledge:manage') || retrieval.isPending} onChange={(event) => setQuery(event.target.value)} placeholder={t('knowledge.retrievalQueryPlaceholder')} value={query} />
          <Input className="w-20" disabled={retrieval.isPending} min={1} max={20} onChange={(event) => setTopK(event.target.value)} type="number" value={topK} />
          <Button disabled={!auth.hasPermission('knowledge:manage') || !query.trim() || retrieval.isPending} onClick={() => retrieval.mutate()} size="sm">{t('knowledge.retrievalRun')}</Button>
        </div>
        {hits && (hits.length ? hits.map((chunk, index) => <div className="rounded-md border border-border bg-muted/40 p-3 text-[11px]" key={index}>
          <p className="whitespace-pre-wrap break-words">{typeof chunk.content === 'string' ? chunk.content : JSON.stringify(chunk.content)}</p>
          <p className="mt-2 text-[10px] text-muted-foreground">{String(chunk.documentId ?? '—')} · {String(chunk.chunkId ?? '—')} · {String(chunk.score ?? '—')}</p>
        </div>) : <p className="text-xs text-muted-foreground">{t('knowledge.retrievalNoHits')}</p>)}
      </div>
    </Card>
    <ConfirmDialog open={Boolean(removeId)} title={t('knowledge.deleteDocument')} description={t('knowledge.deleteDocumentDescription')} cancelLabel={t('common.cancel')} confirmLabel={t('common.confirm')} pending={remove.isPending} onClose={() => setRemoveId(undefined)} onConfirm={() => remove.mutateAsync(removeId!).then(() => undefined)} />
  </ResourceDetailLayout>
}

function documentStatusLabel(t: (key: string) => string, status: string) {
  const labels: Record<string, string> = {
    uploading: t('knowledge.document_uploading'),
    indexing: t('knowledge.document_indexing'),
    indexed: t('knowledge.document_indexed'),
    failed: t('knowledge.document_failed'),
  }
  return labels[status] ?? status
}

function documentStatus(status: string): 'active' | 'pending' | 'running' | 'failed' | 'inactive' {
  if (status === 'indexed') return 'active'
  if (status === 'failed') return 'failed'
  if (status === 'indexing' || status === 'uploading') return 'running'
  return 'pending'
}
