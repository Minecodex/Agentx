import { useMutation, useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { ApiClientError, apiRequest, jsonBody } from '../../shared/api/client'
import type { Knowledge, KnowledgeDocument, PageResponse, RetrievalTestResult } from '../../shared/api/types'
import { Button } from '../../shared/ui/button'
import { Dialog, DialogContent } from '../../shared/ui/dialog'
import { Input } from '../../shared/ui/input'
import { Select } from '../../shared/ui/select'
import { scopeLabel } from './scope'

type Source = RetrievalTestResult['sources'][number]
type Result = RetrievalTestResult
export function QueryInspectDialog({ query, mode, onClose }: { query: Knowledge; mode: 'test' | 'documents'; onClose: () => void }) {
  const { t } = useTranslation()
  const [text, setText] = useState(''); const [topK, setTopK] = useState('5'); const [page, setPage] = useState(1)
  const [dataset, setDataset] = useState(query.scope.kind === 'ragflow' ? query.scope.datasetIds[0] : '')
  const docs = useQuery({ enabled: mode === 'documents', queryKey: ['knowledge-documents', query.id, dataset, page], queryFn: () => apiRequest<PageResponse<KnowledgeDocument>>(`/knowledge/queries/${query.id}/documents?page=${page}&pageSize=20${dataset ? `&datasetId=${encodeURIComponent(dataset)}` : ''}`) })
  const test = useMutation({ mutationFn: () => apiRequest<Result>(`/knowledge/queries/${query.id}/retrieval-test`, { method: 'POST', body: jsonBody({ query: text.trim(), topK: Number(topK) }) }) })
  const result = test.data
  const errorSources = test.error instanceof ApiClientError ? (test.error.detail.details as { sources?: Source[] } | undefined)?.sources : undefined
  const sources = result?.sources ?? errorSources ?? []
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}><DialogContent className="w-[min(960px,calc(100vw-48px))] p-6" title={t(mode === 'test' ? 'knowledge.retrievalTest' : 'knowledge.documents')}>
    <div className="flex items-center justify-between"><h2 className="text-lg font-semibold">{query.name} · {t(mode === 'test' ? 'knowledge.retrievalTest' : 'knowledge.documents')}</h2><Button variant="ghost" onClick={onClose}>{t('common.cancel')}</Button></div>
    <p className="mt-2 text-xs text-muted-foreground">{query.description || scopeLabel(query.scope, t('knowledge.defaultWorkspace'))}</p>
    {mode === 'documents' ? <div className="mt-5 space-y-4">
      <p className="text-xs text-muted-foreground">{t('knowledge.externalManaged')}</p>
      {query.scope.kind === 'ragflow' && <Select aria-label={t('knowledge.datasets')} value={dataset} onValueChange={value => { setDataset(value); setPage(1) }} options={query.scope.datasetIds.map(id => ({ value: id, label: id }))} />}
      {docs.isLoading && <p>{t('common.loading')}</p>}{docs.error && <p role="alert" className="text-xs text-danger">{docs.error.message}</p>}
      <div className="divide-y divide-border rounded-lg border border-border">{docs.data?.items.map(doc => <div className="flex items-center justify-between gap-4 p-3 text-xs" key={doc.id}><div><p className="font-medium">{doc.name}</p><p className="mt-1 text-muted-foreground">{doc.id}</p></div><span>{doc.status}</span></div>)}</div>
      {docs.data?.items.length === 0 && <p className="text-xs text-muted-foreground">{t('knowledge.noDocuments')}</p>}
      <div className="flex justify-end gap-2"><Button variant="secondary" disabled={page === 1} onClick={() => setPage(page - 1)}>{t('knowledge.previous')}</Button><Button variant="secondary" disabled={!docs.data || page * 20 >= docs.data.total} onClick={() => setPage(page + 1)}>{t('knowledge.next')}</Button></div>
    </div> : <div className="mt-5 space-y-4">
      <form className="flex gap-2" onSubmit={event => { event.preventDefault(); test.mutate() }}>
        <Input className="min-w-0 flex-1" aria-label={t('knowledge.retrievalQueryPlaceholder')} placeholder={t('knowledge.retrievalQueryPlaceholder')} value={text} onChange={event => setText(event.target.value)} maxLength={2000} required />
        <Input aria-label="Top K" className="w-20 shrink-0" type="number" min={1} max={20} value={topK} onChange={event => setTopK(event.target.value)} required />
        <Button className="shrink-0 whitespace-nowrap" type="submit" disabled={test.isPending || !text.trim() || Number(topK) < 1 || Number(topK) > 20}>{t('knowledge.retrievalRun')}</Button>
      </form>
      {test.error && <p role="alert" className="text-xs text-danger">{test.error.message}</p>}
      {result && <p className={`text-xs ${result.partial ? 'text-warning' : 'text-muted-foreground'}`}>{t(result.partial ? 'knowledge.partial' : 'knowledge.complete')} · {result.elapsedMs} ms</p>}
      <div className="space-y-2">{sources.map((source, index) => <div className={`rounded-lg border p-3 text-xs ${source.status === 'failed' ? 'border-danger/30 text-danger' : 'border-border text-muted-foreground'}`} key={index}>
        <p>{t('knowledge.source')}: {scopeLabel(source.scope, t('knowledge.defaultWorkspace'))} · {source.elapsedMs ?? 0} ms</p>
        <p className="mt-1 break-words" title={`${source.connectionId} · ${source.queryIds.join(', ')}`}>{query.connectionName} · {query.name}</p>
        {source.error && <p className="mt-1">{source.error.code}: {source.error.message}</p>}
      </div>)}</div>
      {result?.documents.length === 0 && <p className="text-xs text-muted-foreground">{t('knowledge.retrievalNoHits')}</p>}
      {result?.documents.map((hit, index) => <article className="rounded-lg border border-border bg-muted/30 p-4 text-xs" key={index}>
        <h3 className="font-medium">{hit.title ?? hit.documentId ?? t('knowledge.source')}</h3><p className="mt-2 whitespace-pre-wrap break-words">{hit.content}</p>
        <p className="mt-3 text-[11px] text-muted-foreground">{hit.documentId} · {hit.chunkId} · RRF {hit.fusionScore.toFixed(5)}</p>
        {hit.sources.map((source, n) => <p className="mt-1 text-[11px] text-muted-foreground" key={n}>{scopeLabel(source.scope, t('knowledge.defaultWorkspace'))} · {t('knowledge.rawScore')}: {source.rawScore ?? '—'}</p>)}
      </article>)}
    </div>}
  </DialogContent></Dialog>
}
