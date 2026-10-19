import { useQuery } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest } from '../../../shared/api/client'
import type { NodeExecution, RuntimeDetails } from '../../../shared/api/types'
import { useNodeNames } from '../api/node-display'
import { Button } from '../../../shared/ui/button'
import { Dialog, DialogContent } from '../../../shared/ui/dialog'
import { Select } from '../../../shared/ui/select'
import { Textarea } from '../../../shared/ui/textarea'

import { WaitInputsEditor, type WaitInputNode, type WaitInputs } from '../../../shared/components/wait-inputs-editor'

export type DebugInputSource =
  | { kind: 'manual'; value: unknown }
  | { kind: 'history_output'; executionId: string; nodeExecutionId: string; outputPort: string }
  | { kind: 'artifact'; executionId: string; artifactId: string }

export function DebugRunDialog({ open, mode, targetName, executionId, running, waitNodes = [], discoveryPending = false, discoveryError, onClose, onRun }: { open: boolean; mode: 'single_node' | 'from_node'; targetName: string; executionId?: string; running: boolean; waitNodes?: readonly WaitInputNode[]; discoveryPending?: boolean; discoveryError?: string; onClose: () => void; onRun: (source: DebugInputSource, input: unknown, waitInputs: WaitInputs) => void }) {
  const { t } = useTranslation()
  const { resolveNodeName } = useNodeNames()
  const [repliesValid, setRepliesValid] = useState(true);
  const [waitInputs, setWaitInputs] = useState<WaitInputs>({})
  const [kind, setKind] = useState<DebugInputSource['kind']>('manual')
  const [manual, setManual] = useState('{}')
  const [historyNode, setHistoryNode] = useState('')
  const [artifactId, setArtifactId] = useState('')
  const [error, setError] = useState('')
  const nodes = useQuery({ queryKey: ['debug-input-nodes', executionId], queryFn: () => apiRequest<{ items: NodeExecution[] }>(`/executions/${executionId}/nodes`), enabled: open && Boolean(executionId), retry: false })
  const details = useQuery({ queryKey: ['debug-input-artifacts', executionId], queryFn: () => apiRequest<RuntimeDetails>(`/executions/${executionId}/runtime-details`), enabled: open && Boolean(executionId), retry: false })
  const historyOptions = (nodes.data?.items ?? []).filter((node) => node.status === 'succeeded' && node.output !== null && node.output !== undefined).map((node) => ({ value: node.id, label: `${resolveNodeName(node.nodeName, node.nodeType)} · ${t('executions.nodePanel.run')} ${node.runIndex}` }))
  const artifactOptions = useMemo(() => runtimeArtifacts(details.data).map((item, index) => ({ value: item, label: t('studio.debug.artifactLabel', { index: index + 1, id: item.slice(0, 8) }) })), [details.data, t])
  useEffect(() => { if (open) { setError(''); setWaitInputs({}); setRepliesValid(true) } }, [open])

  const submit = () => {
    if (!repliesValid || discoveryPending || discoveryError) return;
    try {
      if (kind === 'manual') {
        const value = JSON.parse(manual)
        onRun({ kind, value }, value, waitInputs)
      } else if (kind === 'history_output') {
        if (!executionId || !historyNode) throw new Error(t('studio.debug.selectHistory'))
        onRun({ kind, executionId, nodeExecutionId: historyNode, outputPort: 'main' }, {}, waitInputs)
      } else {
        if (!executionId || !artifactId) throw new Error(t('studio.debug.selectArtifact'))
        onRun({ kind, executionId, artifactId }, {}, waitInputs)
      }
    } catch (nextError) { setError((nextError as Error).message) }
  }

  return <Dialog onOpenChange={(value) => !value && onClose()} open={open}><DialogContent description={t(mode === 'single_node' ? 'studio.debug.descriptionSingle' : 'studio.debug.descriptionFrom')} title={t('studio.debug.title')}><div className="p-5"><h2 className="text-sm font-semibold">{t('studio.debug.title')} · {targetName}</h2><div className="mt-4 space-y-3"><Field label={t('studio.debug.source')}><Select className="w-full" onValueChange={(value) => setKind(value as DebugInputSource['kind'])} options={[{ value: 'manual', label: t('studio.debug.manual') }, { value: 'history_output', label: t('studio.debug.history'), disabled: !executionId }, { value: 'artifact', label: t('studio.debug.artifact'), disabled: !executionId }]} value={kind} /></Field>
    {kind === 'manual' && <Field label={t('studio.debug.input')}><Textarea className="min-h-40 font-mono text-[11px]" onChange={(event) => setManual(event.target.value)} value={manual} /></Field>}
    {kind === 'history_output' && <Field label={t('studio.debug.nodeOutput')}><Select className="w-full" onValueChange={setHistoryNode} options={historyOptions} placeholder={t('studio.debug.nodeOutputPlaceholder')} value={historyNode} /></Field>}
    {kind === 'artifact' && <Field label={t('studio.debug.artifact')}><Select className="w-full" onValueChange={setArtifactId} options={artifactOptions} placeholder={t('studio.debug.artifactPlaceholder')} value={artifactId} /></Field>}
    <WaitInputsEditor onValidityChange={setRepliesValid} nodes={waitNodes} value={waitInputs} onChange={setWaitInputs} disabled={running} />
    {discoveryError && <p role="alert" className="text-xs text-danger">{discoveryError}</p>}
    {discoveryPending && <p className="text-xs text-muted-foreground">{t('common.loading')}</p>}
    {error && <p className="text-xs text-danger">{error}</p>}</div><div className="mt-5 flex justify-end gap-2"><Button onClick={onClose} variant="ghost">{t('studio.debug.cancel')}</Button><Button disabled={running || !repliesValid || discoveryPending || Boolean(discoveryError)} onClick={submit}>{t('studio.debug.run')}</Button></div></div></DialogContent></Dialog>
}

function runtimeArtifacts(details?: RuntimeDetails) {
  const values = [
    ...(details?.agentRuns ?? []).map((item) => item.stateArtifactId),
    ...(details?.iterations ?? []).map((item) => item.stateArtifactId),
    ...(details?.calls ?? []).map((item) => item.responseArtifactId),
  ].filter((value): value is string => Boolean(value))
  return [...new Set(values)]
}

function Field({ label, children }: { label: string; children: React.ReactNode }) { return <label className="block text-xs"><span className="mb-1.5 block text-muted-foreground">{label}</span>{children}</label> }
