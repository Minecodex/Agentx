import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Activity, ChevronRight, RefreshCw, Trash2 } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest, jsonBody } from '../../shared/api/client'
import { ConfirmDialog } from '../../shared/components/confirm-dialog'
import { EndUserLabel } from '../../shared/components/end-user-label'
import { EmptyState } from '../../shared/components/empty-state'
import { PageContainer } from '../../shared/components/page-container'
import { PageHeader } from '../../shared/components/page-header'
import { StatusBadge } from '../../shared/components/status-badge'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { Input } from '../../shared/ui/input'
import { Table, TableCell, TableContainer, TableHead } from '../../shared/ui/table'
import { useToast } from '../../shared/ui/toast'

type SessionSummary = {
  sessionKey: string
  sessionId: string
  stableAgentNodeKey: string
  applicationId?: string | null
  stateVersion: number
  fencingToken: number
  leafEntryId?: string | null
  openOperationId?: string | null
  terminalState?: string | null
  bundleHash: string
  modelVersion: string
  updatedAt: string
}
type SessionPage = { items: SessionSummary[]; next?: string | null }
type SessionEntry = { entryId: string; entryKind: string; operationId?: string | null; payload?: Record<string, unknown> | null; createdAt: string }
type MemoryTarget = { memoryResourceId: string; scope: { tenantId: string; applicationId: string; environmentId?: string | null; workflowId: string; stableNodeId: string; user: { source: string; externalUserId: string } } }
type SessionDiagnostic = { memoryTargets?: MemoryTarget[]; summary: SessionSummary; entries: SessionEntry[]; usages: Array<Record<string, unknown>>; compaction?: unknown; recovery?: Record<string, unknown> | null }
type MemoryAuditPage = { items: Array<{ auditId: string; operation: string; scopeHash: string; operationId?: string | null; createdAt: string }>; next?: string | null }

export function AgentSessionsPage() {
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  const { showToast } = useToast()
  const queryClient = useQueryClient()
  const [applicationId, setApplicationId] = useState('')
  const [sessionKey, setSessionKey] = useState('')
  const [nodeKey, setNodeKey] = useState('')
  const [selected, setSelected] = useState<{ sessionKey: string; nodeKey: string }>()
  const [clearOpen, setClearOpen] = useState(false)

  const query = useMemo(() => {
    const params = new URLSearchParams({ limit: '50' })
    if (applicationId.trim()) params.set('applicationId', applicationId.trim())
    if (sessionKey.trim()) params.set('sessionKey', sessionKey.trim())
    if (nodeKey.trim()) params.set('stableAgentNodeKey', nodeKey.trim())
    return params.toString()
  }, [applicationId, nodeKey, sessionKey])
  const sessions = useQuery({ queryKey: ['agent-sessions', query], queryFn: () => apiRequest<SessionPage>(`/agent-sessions?${query}`), retry: false })
  const detail = useQuery({
    queryKey: ['agent-session', selected?.sessionKey, selected?.nodeKey],
    queryFn: () => apiRequest<SessionDiagnostic>(`/agent-sessions/${encodeURIComponent(selected!.sessionKey)}/${encodeURIComponent(selected!.nodeKey)}`),
    enabled: Boolean(selected),
    retry: false,
  })
  const clear = useMutation({
    mutationFn: () => apiRequest('/agent-sessions/clear', { method: 'POST', body: jsonBody({ sessionKey: selected?.sessionKey, stableAgentNodeKey: selected?.nodeKey, idempotencyKey: `web:${selected?.sessionKey}:${Date.now()}` }) }),
    onSuccess: async () => {
      setClearOpen(false)
      setSelected(undefined)
      await queryClient.invalidateQueries({ queryKey: ['agent-sessions'] })
      showToast(t('agentSessions.clearSuccess'))
    },
  })

  return <PageContainer>
    <PageHeader description={t('agentSessions.description')} title={t('agentSessions.title')} />
    <Card className="mt-6 overflow-hidden">
      <div className="flex flex-wrap items-end gap-3 border-b border-border p-4">
        <FilterField label={t('agentSessions.applicationId')} onChange={setApplicationId} value={applicationId} />
        <FilterField label={t('agentSessions.sessionKey')} onChange={setSessionKey} value={sessionKey} />
        <FilterField label={t('agentSessions.nodeKey')} onChange={setNodeKey} value={nodeKey} />
        <Button aria-label={t('agentSessions.refresh')} className="mb-0.5" onClick={() => void sessions.refetch()} size="icon" variant="secondary"><RefreshCw className="size-4" /></Button>
      </div>
      {sessions.isLoading ? <Loading /> : sessions.isError ? <ErrorState message={sessions.error.message} /> : sessions.data?.items.length ? <TableContainer><Table className="min-w-[900px]"><thead className="bg-muted/45 text-[10px] uppercase tracking-[0.08em] text-muted-foreground"><tr><TableHead>{t('agentSessions.sessionKey')}</TableHead><TableHead>{t('agentSessions.nodeKey')}</TableHead><TableHead>{t('agentSessions.state')}</TableHead><TableHead>{t('agentSessions.version')}</TableHead><TableHead>{t('agentSessions.model')}</TableHead><TableHead>{t('agentSessions.updated')}</TableHead><TableHead /></tr></thead><tbody>{sessions.data.items.map((item) => <tr className="border-t border-border hover:bg-muted/30" key={`${item.sessionKey}:${item.stableAgentNodeKey}`}><TableCell className="font-medium text-foreground">{item.sessionKey}</TableCell><TableCell>{item.stableAgentNodeKey}</TableCell><TableCell><StatusBadge label={item.openOperationId ? t('agentSessions.open') : item.terminalState ?? t('agentSessions.idle')} status={item.openOperationId ? 'running' : item.terminalState === 'failed' ? 'failed' : 'completed'} /></TableCell><TableCell>#{item.stateVersion} · F{item.fencingToken}</TableCell><TableCell className="max-w-56 truncate" title={item.modelVersion}>{item.modelVersion}</TableCell><TableCell>{formatDateTime(item.updatedAt)}</TableCell><TableCell><Button aria-label={t('agentSessions.inspect')} onClick={() => setSelected({ sessionKey: item.sessionKey, nodeKey: item.stableAgentNodeKey })} size="icon" variant="ghost"><ChevronRight className="size-4" /></Button></TableCell></tr>)}</tbody></Table></TableContainer> : <EmptyState title={t('agentSessions.empty')} description={t('agentSessions.emptyDescription')} icon={Activity} />}
    </Card>
    {selected && <SessionDetail detail={detail.data} error={detail.error} formatDateTime={formatDateTime} loading={detail.isLoading} onClear={() => setClearOpen(true)} onClose={() => setSelected(undefined)} t={t} />}
    <ConfirmDialog cancelLabel={t('common.cancel')} confirmLabel={t('agentSessions.clear')} description={t('agentSessions.clearDescription')} onClose={() => setClearOpen(false)} onConfirm={() => clear.mutateAsync().then(() => undefined)} open={clearOpen} pending={clear.isPending} title={t('agentSessions.clearTitle')} />
  </PageContainer>
}

function FilterField({ label, onChange, value }: { label: string; onChange: (value: string) => void; value: string }) {
  return <label className="min-w-52 flex-1 text-[11px] font-medium text-muted-foreground">{label}<Input className="mt-1.5 h-8 text-xs" onChange={(event) => onChange(event.target.value)} value={value} /></label>
}

function SessionDetail({ detail, error, formatDateTime, loading, onClear, onClose, t }: { detail?: SessionDiagnostic; error: Error | null; formatDateTime: (value: string) => string; loading: boolean; onClear: () => void; onClose: () => void; t: (key: string, options?: Record<string, unknown>) => string }) {
  return <Card className="mt-6 overflow-hidden">
    <div className="flex flex-wrap items-center gap-2 border-b border-border px-5 py-4"><Activity className="size-4 text-primary" /><h2 className="text-sm font-semibold">{t('agentSessions.detail')}</h2><span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">{detail?.summary.sessionKey} / {detail?.summary.stableAgentNodeKey}</span><Button onClick={onClose} size="sm" variant="ghost">{t('common.close')}</Button>{detail && <Button onClick={onClear} size="sm" variant="danger"><Trash2 className="size-3.5" />{t('agentSessions.clear')}</Button>}</div>
    {loading ? <Loading /> : error ? <ErrorState message={error.message} /> : detail ? <div className="grid gap-5 p-5 lg:grid-cols-2"><InfoSection title={t('agentSessions.summary')}><Info label={t('agentSessions.state')} value={`#${detail.summary.stateVersion} · F${detail.summary.fencingToken}`} /><Info label={t('agentSessions.bundle')} value={detail.summary.bundleHash} /><Info label={t('agentSessions.model')} value={detail.summary.modelVersion} /><Info label={t('agentSessions.updated')} value={formatDateTime(detail.summary.updatedAt)} /></InfoSection><InfoSection title={t('agentSessions.compaction')}><pre className="max-h-44 overflow-auto whitespace-pre-wrap break-all text-[11px] text-muted-foreground">{detail.compaction ? JSON.stringify(detail.compaction, null, 2) : t('agentSessions.none')}</pre></InfoSection><InfoSection title={t('agentSessions.recovery')}><pre className="max-h-44 overflow-auto whitespace-pre-wrap break-all text-[11px] text-muted-foreground">{detail.recovery ? JSON.stringify(detail.recovery, null, 2) : t('agentSessions.none')}</pre></InfoSection><InfoSection title={t('agentSessions.usage')}><UsageTable formatDateTime={formatDateTime} items={detail.usages} t={t} /></InfoSection><InfoSection className="lg:col-span-2" title={t('agentSessions.entries')}><EntriesTable formatDateTime={formatDateTime} items={detail.entries} t={t} /></InfoSection><InfoSection className="lg:col-span-2" title={t('agentSessions.memoryAudit')}>{detail.memoryTargets?.map(target => <MemoryAudit key={target.memoryResourceId} target={target} formatDateTime={formatDateTime} t={t} />)}{!detail.memoryTargets?.length && <p className="text-xs text-muted-foreground">{t('agentSessions.none')}</p>}</InfoSection></div> : null}
  </Card>
}

function MemoryAudit({ target, formatDateTime, t }: { target: MemoryTarget; formatDateTime: (value: string) => string; t: (key: string) => string }) {
  const [requested, setRequested] = useState(false)
  const body = { applicationId: target.scope.applicationId, memoryResourceId: target.memoryResourceId, scope: target.scope }
  const memory = useQuery({ queryKey: ['agent-subject-memory-audit', target], queryFn: () => apiRequest<MemoryAuditPage>('/agent-subject-memory/audit', { method: 'POST', body: jsonBody({ ...body, limit: 50 }) }), enabled: requested, retry: false })
  const clear = useMutation({ mutationFn: () => apiRequest('/agent-subject-memory/clear', { method: 'POST', body: jsonBody({ ...body, idempotencyKey: `web:${Date.now()}` }) }), onSuccess: () => { void memory.refetch() } })
  return <div className="space-y-3"><div className="flex items-center gap-3"><EndUserLabel user={target.scope.user} /><span className="flex-1 truncate text-xs text-muted-foreground">{target.scope.stableNodeId}</span><Button onClick={() => setRequested(true)} size="sm" variant="secondary">{t('agentSessions.loadAudit')}</Button><Button disabled={clear.isPending} onClick={() => void clear.mutateAsync()} size="sm" variant="danger">{t('agentSessions.clearMemory')}</Button></div>{clear.error && <ErrorState message={clear.error.message} />}{requested && (memory.isLoading ? <Loading /> : memory.error ? <ErrorState message={memory.error.message} /> : <EntriesTable formatDateTime={formatDateTime} items={(memory.data?.items ?? []).map(item => ({ entryId: item.auditId, entryKind: item.operation, operationId: item.operationId, createdAt: item.createdAt }))} t={t} />)}</div>
}

function InfoSection({ children, className = '', title }: { children: React.ReactNode; className?: string; title: string }) { return <section className={`rounded-lg border border-border p-4 ${className}`}><h3 className="mb-3 text-xs font-semibold">{title}</h3>{children}</section> }
function Info({ label, value }: { label: string; value: string }) { return <div className="grid grid-cols-[120px_minmax(0,1fr)] gap-2 border-t border-border py-2 text-xs first:border-t-0"><span className="text-muted-foreground">{label}</span><span className="truncate" title={value}>{value}</span></div> }
function EntriesTable({ formatDateTime, items, t }: { formatDateTime: (value: string) => string; items: Array<{ entryId: string; entryKind: string; operationId?: string | null; createdAt: string; payload?: Record<string, unknown> | null }>; t: (key: string, options?: Record<string, unknown>) => string }) { return items.length ? <TableContainer><Table className="min-w-[520px]"><thead className="text-[10px] uppercase tracking-[0.08em] text-muted-foreground"><tr><TableHead>{t('agentSessions.entryKind')}</TableHead><TableHead>{t('agentSessions.operation')}</TableHead><TableHead>{t('agentSessions.created')}</TableHead></tr></thead><tbody>{items.map((item) => <tr className="border-t border-border" key={item.entryId}><TableCell>{item.entryKind}</TableCell><TableCell className="max-w-64 truncate" title={item.operationId ?? ''}>{item.operationId ?? '—'}</TableCell><TableCell>{formatDateTime(item.createdAt)}</TableCell></tr>)}</tbody></Table></TableContainer> : <p className="text-xs text-muted-foreground">{t('agentSessions.none')}</p> }
function UsageTable({ formatDateTime, items, t }: { formatDateTime: (value: string) => string; items: Array<Record<string, unknown>>; t: (key: string, options?: Record<string, unknown>) => string }) { return items.length ? <TableContainer><Table className="min-w-[520px]"><thead className="text-[10px] uppercase tracking-[0.08em] text-muted-foreground"><tr><TableHead>{t('agentSessions.kind')}</TableHead><TableHead>{t('agentSessions.tokens')}</TableHead><TableHead>{t('agentSessions.cost')}</TableHead><TableHead>{t('agentSessions.created')}</TableHead></tr></thead><tbody>{items.map((item, index) => <tr className="border-t border-border" key={`${String(item.effectId)}:${index}`}><TableCell>{String(item.usageKind ?? '—')}</TableCell><TableCell>{String(item.inputTokens ?? 0)} / {String(item.outputTokens ?? 0)}</TableCell><TableCell>{String(item.costMicros ?? 0)} {String(item.costCurrency ?? '')}</TableCell><TableCell>{item.createdAt ? formatDateTime(String(item.createdAt)) : '—'}</TableCell></tr>)}</tbody></Table></TableContainer> : <p className="text-xs text-muted-foreground">{t('agentSessions.none')}</p> }
function Loading() { return <p className="p-8 text-center text-sm text-muted-foreground">Loading…</p> }
function ErrorState({ message }: { message: string }) { return <p className="p-8 text-center text-sm text-danger">{message}</p> }
