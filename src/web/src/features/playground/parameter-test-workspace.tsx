import { WaitInputsEditor, useApplicationWaitNodes, type WaitInputs } from '../../shared/components/wait-inputs-editor'
import { WorkflowInteractions } from '../../shared/components/workflow-interactions'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ExternalLink, FileDown, Play } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'

import { gatewayRequest, jsonBody, apiRequest } from '../../shared/api/client'
import type { Application, Execution, GatewayInvocation } from '../../shared/api/types'
import { SchemaFormWorkspace, schemaDefaults, validateSchemaValues, type ArtifactReference } from '../../shared/components/schema-form'
import { Badge } from '../../shared/ui/badge'
import { Button } from '../../shared/ui/button'
import { localizedValue } from '../../shared/lib/localized-value'
import type { PlaygroundDeployment } from './playground-types'
import { projectHistoricalInput } from './playground-schema-values'
import { useInvocationEvents } from './use-invocation-events'
import { useExecutionArtifactDownload } from '../traces/use-execution-artifact-download'

type ExecutionPage = { items: Execution[]; limit: number; total: number; nextCursor?: string | null }
const terminal = new Set(['completed', 'failed', 'cancelled'])

export function ParameterTestWorkspace({ application, deployment, uploadArtifact }: { application: Application; deployment: PlaygroundDeployment; uploadArtifact: (file: File) => Promise<ArtifactReference> }) {
  const { t, i18n } = useTranslation()
  const queryClient = useQueryClient()
  const waitNodes = useApplicationWaitNodes(application.id, deployment.id, deployment.workflowVersionId)
  const [repliesValid, setRepliesValid] = useState(true);
  const [waitInputs, setWaitInputs] = useState<WaitInputs>({})
  const [values, setValues] = useState<Record<string, unknown>>(() => schemaDefaults(deployment.inputSchema))
  const [errors, setErrors] = useState<Record<string, string>>({})
  const [invocationId, setInvocationId] = useState<string>()
  const [executionId, setExecutionId] = useState<string>()
  useEffect(() => { setValues(schemaDefaults(deployment.inputSchema)); setWaitInputs({}); setErrors({}); setInvocationId(undefined); setExecutionId(undefined) }, [deployment.id, deployment.inputSchema])
  const history = useQuery({ queryKey: ['playground-executions', application.id], queryFn: () => apiRequest<ExecutionPage>(`/executions?applicationIds=${application.id}&sessionMode=stateless&limit=50`) })
  const invocation = useQuery({ queryKey: ['gateway-invocation', invocationId], queryFn: () => gatewayRequest<GatewayInvocation>(`/invocations/${invocationId}`), enabled: Boolean(invocationId), refetchInterval: (query) => query.state.data && terminal.has(query.state.data.status) ? false : 2_000 })
  const executionDetail = useQuery({ queryKey: ['playground-execution-detail', executionId], queryFn: () => apiRequest<Execution>(`/executions/${executionId}`), enabled: Boolean(executionId && !invocationId) })
  const running = Boolean(invocation.data && !terminal.has(invocation.data.status))
  useInvocationEvents(invocationId, running, () => { void queryClient.invalidateQueries({ queryKey: ['gateway-invocation', invocationId] }); void queryClient.invalidateQueries({ queryKey: ['playground-executions', application.id] }) })
  useEffect(() => {
    if (!invocationId && executionDetail.data && executionDetail.data.id === executionId) {
      setValues(projectHistoricalInput(deployment.inputSchema, executionDetail.data.input))
      setErrors({})
    }
  }, [deployment.inputSchema, executionDetail.data, executionId, invocationId])
  useEffect(() => {
    if (invocation.data && terminal.has(invocation.data.status)) {
      void queryClient.invalidateQueries({ queryKey: ['playground-executions', application.id] })
    }
  }, [application.id, invocation.data, queryClient])
  const invoke = useMutation({
    mutationFn: () => gatewayRequest<GatewayInvocation>(`/applications/${application.slug}/invocations`, { method: 'POST', headers: { 'Idempotency-Key': crypto.randomUUID() }, body: jsonBody({ input: values, sessionId: null, responseMode: 'async', waitInputs }) }),
    onSuccess: (result) => { setInvocationId(result.id); setExecutionId(result.executionId ?? undefined) },
  })
  const run = () => { const next = validateSchemaValues(deployment.inputSchema, values); setErrors(next); if (Object.keys(next).length === 0 && repliesValid && !waitNodes.isPending && !waitNodes.error) invoke.mutate() }
  const selectHistory = (item: Execution) => { setExecutionId(item.id); setInvocationId(undefined) }
  const selectedExecution = history.data?.items.find((item) => item.id === executionId)
  const shownOutput = invocation.data?.outputs ?? executionDetail.data?.output
  const shownError = invocation.data?.error ?? executionDetail.data?.error
  const artifacts = useMemo(() => collectArtifacts(shownOutput), [shownOutput])
  const download = useExecutionArtifactDownload(executionId)
  return <div className="grid h-full min-h-0 grid-cols-[260px_minmax(360px,1fr)_minmax(300px,0.85fr)] overflow-hidden rounded-lg border border-border bg-surface">
    <aside className="flex min-h-0 flex-col border-r border-border bg-muted/20"><PanelTitle>{t('applications.playground.runHistory')}</PanelTitle><div className="min-h-0 flex-1 space-y-1 overflow-auto p-2">{history.isLoading ? <Hint>{t('common.loading')}</Hint> : history.data?.items.length ? history.data.items.map((item) => <button aria-pressed={item.id === executionId} className={`w-full rounded-md p-3 text-left ${item.id === executionId ? 'bg-primary/10' : 'hover:bg-muted'}`} key={item.id} onClick={() => selectHistory(item)} type="button"><span className="flex items-center gap-2"><strong className="min-w-0 flex-1 truncate text-xs">{new Date(item.startedAt).toLocaleString(i18n.resolvedLanguage)}</strong><Badge tone={item.status === 'succeeded' ? 'success' : item.status === 'failed' ? 'danger' : 'warning'}>{localizedValue(t, 'common', item.status)}</Badge></span><span className="mt-1 block truncate font-mono text-[9px] text-muted-foreground">{item.id}</span></button>) : <Hint>{t('applications.playground.noRuns')}</Hint>}</div></aside>
    <section className="flex min-h-0 min-w-0 flex-col border-r border-border"><div className="flex h-14 shrink-0 items-center justify-between border-b border-border px-5"><div><h2 className="text-sm font-semibold">{t('applications.playground.inputParameters')}</h2><p className="text-[10px] text-muted-foreground">{t('applications.playground.deploymentVersion', { deployment: deployment.sequenceNumber, workflow: deployment.workflowVersionNumber })}</p></div><Button disabled={running || invoke.isPending || !repliesValid || waitNodes.isPending || Boolean(waitNodes.error)} onClick={run} size="sm"><Play className="size-3.5" />{t('applications.playground.runTest')}</Button></div><div className="min-h-0 flex-1 overflow-auto p-5">{waitNodes.error && <p role="alert" className="mb-2 text-xs text-danger">{waitNodes.error.message}<Button size="sm" variant="ghost" onClick={() => void waitNodes.refetch()}>{t('common.retry')}</Button></p>}<WaitInputsEditor onValidityChange={setRepliesValid} nodes={waitNodes.data ?? []} value={waitInputs} onChange={setWaitInputs} disabled={running || invoke.isPending} /><SchemaFormWorkspace disabled={running} onChange={setValues} onUpload={uploadArtifact} schema={deployment.inputSchema} value={values} />{Object.keys(errors).length > 0 && <div className="mt-4 rounded-md bg-danger/10 p-3 text-xs text-danger">{Object.entries(errors).map(([field, message]) => <p key={field}>{field}: {message}</p>)}</div>}{invoke.error && <p className="mt-4 text-xs text-danger">{invoke.error.message}</p>}</div></section>
    <section className="flex min-h-0 min-w-0 flex-col"><PanelTitle>{t('applications.playground.runResult')}</PanelTitle><div className="min-h-0 flex-1 overflow-auto p-5">{!invocation.data && !selectedExecution ? <div className="grid min-h-80 place-items-center text-xs text-muted-foreground">{t('applications.playground.selectRunHint')}</div> : <div className="space-y-5"><div className="flex items-center gap-2"><Badge tone={(invocation.data?.status ?? selectedExecution?.status) === 'failed' ? 'danger' : running ? 'warning' : 'success'}>{localizedValue(t, 'common', invocation.data?.status ?? selectedExecution?.status ?? 'unknown')}</Badge>{executionId && <Button asChild size="sm" variant="ghost"><Link to={`/executions/${executionId}`}><ExternalLink className="size-3.5" />{t('applications.playground.executionTrace')}</Link></Button>}</div>{selectedExecution && <div className="grid grid-cols-2 gap-3 text-[10px]"><Metric label={t('applications.playground.duration')} value={selectedExecution.durationMs == null ? '—' : `${selectedExecution.durationMs} ms`} /><Metric label={t('applications.playground.token')} value={`${selectedExecution.inputTokens ?? 0} / ${selectedExecution.outputTokens ?? 0}`} /><Metric label={t('applications.playground.cost')} value={`${selectedExecution.costMicros ?? 0} μ`} /><Metric label="Trace" value={selectedExecution.traceId?.slice(0, 12) ?? '—'} /></div>}<WorkflowInteractions messages={invocation.data?.messages} waits={invocation.data?.activeWaits} /><ResultBlock label={t('applications.playground.output')} value={shownOutput} />{shownError != null && <ResultBlock danger label={t('applications.playground.error')} value={shownError} />}{artifacts.length > 0 && <div><h3 className="mb-2 text-xs font-semibold">{t('applications.playground.outputFiles')}</h3><div className="space-y-2">{artifacts.map((artifact) => <button className="flex w-full items-center gap-2 rounded-md border border-border p-3 text-left text-xs hover:bg-muted" key={artifact.artifactId} onClick={() => void download(artifact.artifactId)} type="button"><FileDown className="size-4 text-primary" /><span className="min-w-0 flex-1 truncate">{artifact.fileName ?? artifact.artifactId}</span></button>)}</div></div>}</div>}</div></section>
  </div>
}

function PanelTitle({ children }: { children: React.ReactNode }) { return <div className="flex h-14 shrink-0 items-center border-b border-border px-4 text-sm font-semibold">{children}</div> }
function Hint({ children }: { children: React.ReactNode }) { return <p className="p-4 text-center text-xs text-muted-foreground">{children}</p> }
function Metric({ label, value }: { label: string; value: string }) { return <div className="rounded-md bg-muted/40 p-2"><span className="text-muted-foreground">{label}</span><strong className="mt-1 block font-mono">{value}</strong></div> }
function ResultBlock({ label, value, danger = false }: { label: string; value: unknown; danger?: boolean }) { return <div><h3 className={`mb-2 text-xs font-semibold ${danger ? 'text-danger' : ''}`}>{label}</h3><pre className={`max-h-80 overflow-auto rounded-md p-3 text-[11px] ${danger ? 'bg-danger/10 text-danger' : 'bg-canvas'}`}>{JSON.stringify(value, null, 2)}</pre></div> }
function collectArtifacts(value: unknown): ArtifactReference[] { const found: ArtifactReference[] = []; const visit = (item: unknown) => { if (!item || typeof item !== 'object') return; if (Array.isArray(item)) { item.forEach(visit); return } const record = item as Record<string, unknown>; if (typeof record.artifactId === 'string') found.push(record as ArtifactReference); Object.values(record).forEach(visit) }; visit(value); return [...new Map(found.map((item) => [item.artifactId, item])).values()] }
