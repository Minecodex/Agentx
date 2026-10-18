import { useInfiniteQuery, useMutation } from '@tanstack/react-query'
import { AlertTriangle, ExternalLink, LoaderCircle, Trash2 } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'

import { ApiClientError, apiRequest } from '../api/client'
import type { DeletionImpact, DeletionReference } from '../api/types'
import { Button } from '../ui/button'
import { Dialog, DialogContent } from '../ui/dialog'

type EntityDeleteDialogProps = {
  deletePath: string
  entityId: string
  entityName: string
  entityType: string
  onClose: () => void
  onDeleted: () => Promise<void> | void
  open: boolean
}

export function EntityDeleteDialog({ deletePath, entityId, entityName, entityType, onClose, onDeleted, open }: EntityDeleteDialogProps) {
  const { t } = useTranslation()
  const [conflictImpact, setConflictImpact] = useState<DeletionImpact>()
  const [conflictLoading, setConflictLoading] = useState(false)
  const impact = useInfiniteQuery({
    queryKey: ['deletion-impact', entityType, entityId],
    queryFn: ({ pageParam }) => apiRequest<DeletionImpact>(`/deletion-impact/${entityType}/${entityId}?page=${pageParam}&pageSize=20`),
    initialPageParam: 1,
    enabled: open,
    getNextPageParam: (last, pages) => pages.flatMap((page) => page.references).length < last.total ? last.page + 1 : undefined,
  })
  const current = conflictImpact ?? impact.data?.pages.at(-1)
  const references = useMemo(() => conflictImpact?.references ?? impact.data?.pages.flatMap((page) => page.references) ?? [], [conflictImpact, impact.data])
  const groups = useMemo(() => Object.entries(references.reduce<Record<string, DeletionReference[]>>((result, reference) => {
    result[reference.sourceModule] ??= []
    result[reference.sourceModule].push(reference)
    return result
  }, {})), [references])
  const remove = useMutation({
    mutationFn: () => apiRequest<void>(`${deletePath}?expectedVersion=${current?.targetVersion}`, { method: 'DELETE' }),
    onSuccess: async () => { await onDeleted(); onClose() },
    onError: (error) => {
      if (error instanceof ApiClientError && error.status === 409 && isDeletionImpact(error.detail.details)) {
        setConflictImpact(error.detail.details)
      }
    },
  })
  const loadMore = async () => {
    if (!conflictImpact) {
      await impact.fetchNextPage()
      return
    }
    setConflictLoading(true)
    try {
      const next = await apiRequest<DeletionImpact>(`/deletion-impact/${entityType}/${entityId}?page=${conflictImpact.page + 1}&pageSize=${conflictImpact.pageSize}`)
      setConflictImpact({ ...next, references: [...conflictImpact.references, ...next.references] })
    } finally {
      setConflictLoading(false)
    }
  }
  const close = () => { if (!remove.isPending) { setConflictImpact(undefined); onClose() } }
  const title = t('common.deletion.title', { name: entityName })

  return <Dialog onOpenChange={(next) => !next && close()} open={open}>
    <DialogContent description={t('common.deletion.irreversible')} title={title}>
      <div className="border-b border-border px-5 py-4"><h2 className="flex items-center gap-2 text-sm font-semibold"><Trash2 className="size-4 text-danger" />{title}</h2></div>
      <div className="space-y-4 p-5">
        {impact.isPending && <div className="flex min-h-28 items-center justify-center gap-2 text-xs text-muted-foreground"><LoaderCircle className="size-4 animate-spin" />{t('common.deletion.loading')}</div>}
        {impact.isError && <p className="rounded-md border border-danger/25 bg-danger/5 p-3 text-xs text-danger">{t('common.deletion.loadFailed')}</p>}
        {current?.deletable && <div className="rounded-md border border-danger/25 bg-danger/5 p-4"><p className="text-sm font-medium text-danger">{t('common.deletion.irreversible')}</p><p className="mt-1 text-xs text-muted-foreground">{entityName}</p></div>}
        {current && !current.deletable && <>
          <div className="flex gap-3 rounded-md border border-warning/30 bg-warning/5 p-4"><AlertTriangle className="mt-0.5 size-4 shrink-0 text-warning" /><div><p className="text-sm font-medium">{t('common.deletion.blocked')}</p><p className="mt-1 text-xs text-muted-foreground">{t('common.deletion.references')} · {current.total}</p></div></div>
          <div className="max-h-80 divide-y divide-border overflow-auto rounded-md border border-border">{groups.map(([module, items]) => <section key={module}><h3 className="bg-muted/50 px-3 py-2 text-[10px] font-semibold uppercase text-muted-foreground">{module}</h3>{items?.map((reference) => <ReferenceRow key={`${reference.sourceType}-${reference.sourceId}-${reference.relation}-${reference.nodeId ?? ''}`} reference={reference} />)}</section>)}</div>
          {references.length < current.total && <Button disabled={impact.isFetchingNextPage || conflictLoading} onClick={() => void loadMore()} size="sm" variant="secondary">{(impact.isFetchingNextPage || conflictLoading) && <LoaderCircle className="size-3.5 animate-spin" />}{t('common.deletion.loadMore')}</Button>}
        </>}
        {remove.error && !(remove.error instanceof ApiClientError && remove.error.status === 409) && <p className="text-xs text-danger">{String(remove.error)}</p>}
      </div>
      <div className="flex justify-end gap-2 border-t border-border px-5 py-4"><Button disabled={remove.isPending} onClick={close} variant="ghost">{t('common.cancel')}</Button><Button disabled={!current?.deletable || impact.isFetching || remove.isPending} onClick={() => remove.mutate()} variant="danger">{remove.isPending && <LoaderCircle className="size-3.5 animate-spin" />}{t('common.deletion.confirm')}</Button></div>
    </DialogContent>
  </Dialog>
}

function ReferenceRow({ reference }: { reference: DeletionReference }) {
  const href = referenceHref(reference)
  const content = <><span className="min-w-0"><strong className="block truncate text-xs">{reference.sourceName}</strong><span className="mt-0.5 block truncate text-[10px] text-muted-foreground">{reference.sourceType} · {reference.relation}{reference.nodeName ? ` · ${reference.nodeName}` : reference.nodeId ? ` · ${reference.nodeId}` : ''}</span></span>{href && <ExternalLink className="size-3.5 shrink-0 text-muted-foreground" />}</>
  return href ? <Link className="grid grid-cols-[1fr_auto] items-center gap-3 px-3 py-2.5 hover:bg-muted/40" to={href}>{content}</Link> : <div className="grid grid-cols-[1fr_auto] items-center gap-3 px-3 py-2.5">{content}</div>
}

function referenceHref(reference: DeletionReference) {
  const parent = reference.parentId ?? reference.sourceId
  if (reference.sourceModule === 'applications' && reference.parentId) {
    const root = `/applications/${reference.parentId}?tab=testing`
    if (reference.sourceType === 'dataset') return `${root}&testTab=datasets&datasetId=${reference.sourceId}`
    if (reference.sourceType === 'evaluation_profile') return `${root}&testTab=profiles`
    if (reference.sourceType === 'evaluation_run') return `${root}&testTab=runs&batchId=${reference.sourceId}`
  }
  const routes: Record<string, string> = { workflows: `/workflows/${parent}`, applications: `/applications/${parent}`, executions: `/executions/${parent}`, approvals: `/approvals/${reference.sourceId}`, skills: `/skills/${parent}`, models: `/models/${parent}`, mcp: `/mcp/${parent}`, knowledge: `/knowledge/${parent}`, memory: `/memory/${parent}`, sandbox: `/sandbox-profiles/${parent}`, organization: '/organization', roles: '/roles', resourceGrants: '/resource-grants' }
  return routes[reference.sourceModule]
}

function isDeletionImpact(value: unknown): value is DeletionImpact {
  return Boolean(value && typeof value === 'object' && 'deletable' in value && 'references' in value && 'targetVersion' in value)
}
