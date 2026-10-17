import { useAuth } from '../../../app/providers/auth-provider'
import type { components } from '../../../shared/api/generated'
import { useQuery } from '@tanstack/react-query'
import { Send, Square, X } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest, apiRequestStream, jsonBody } from '../../../shared/api/client'
import { Button } from '../../../shared/ui/button'
import { Select } from '../../../shared/ui/select'
import { MarkdownContent } from '../../../shared/ui/markdown-content'
import { useToast } from '../../../shared/ui/toast'
import { deserializeDraft, serializeStudio } from '../model/serializer'
import { canonicalDocument, documentHash } from '../model/document-hash'
import { restoreBuilderReceipt, type ReceiptProof } from '../model/builder-receipt'
import type { NodeManifest, StudioDocument } from '../model/types'
import { layoutStudioDocument } from '../utils/layout'
import { useEditorStore, useEditorStoreApi } from '../store/editor-store'

type Snapshot = components['schemas']['StudioEditorSnapshotV1']
type Command = components['schemas']['StudioEditorCommandV1']
type Entry = { sequence: number; runId: string | null; payload: { type: string; text?: string; status?: string; commandId?: string; issues?: string[]; questionId?: string; question?: string; options?: string[]; report?: {documentHash:string;issues: Array<{code: string;message:string;nodeId?:string|null;fieldPath?:string|null}>}; command?: Command; message?: { role: string; content: string; isError: boolean }; inputTokens?: number; outputTokens?: number; costMicros?: number; currency?: string } }
type Session = components['schemas']['StudioBuilderSessionV1']
type Ack = components['schemas']['StudioEditorAcknowledgementV1']
const active = new Set(['queued', 'running', 'waiting'])

export function BuilderPanel({ manifests, workflowId, draftId, serverRevision, executionId, onClose, persistRecovery, checkDocument, onLocate }: { manifests: Map<string, NodeManifest>; workflowId: string; draftId: string; serverRevision: number; executionId?: string; onClose: () => void; persistRecovery: () => boolean; checkDocument: (document: StudioDocument) => string[]; onLocate: (issue: {code: string; message: string; nodeId?:string;fieldPath?:string}) => void }) {
  const { t } = useTranslation()
  const auth = useAuth()
  const { showToast } = useToast()
  const store = useEditorStoreApi()
  const dirty = useEditorStore(state => state.dirty)
  const localRevision = useEditorStore(state => state.localDocumentRevision)
  const [currentHash,setCurrentHash] = useState('')
  useEffect(() => {
    let cancelled=false
    void documentHash(serializeStudio(store.getState())).then(hash=>{if(!cancelled)setCurrentHash(hash)})
    return ()=>{cancelled=true}
  },[localRevision,store])
  const selection = useEditorStore(state => state.nodes.filter(node => node.selected || node.id === state.selectedId).map(node => node.id).join('\u0000'))
  const [scope, setScope] = useState('canvas')
  const [prompt, setPrompt] = useState('')
  const [session, setSession] = useState<Session>()
  const [entries, setEntries] = useState<Entry[]>([])
  const [run, setRun] = useState<{ id: string; status: string }>()
  const [liveText, setLiveText] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string>()
  const selectedExecution = useRef<string | null>(null)
  const cursor = useRef(0)
  const instance = useRef('')
  const receipts = useRef<Record<string, Ack>>({})
  const proofs = useRef<Record<string,ReceiptProof>>({})
  const receiptKey = `agentx:builder-receipts:${session?.id ?? ''}`
  const mounted = useRef(true)
  const blockedRuns = useRef(new Set<string>())
  const inFlightRun = useRef<string>()
  const activeBinding = useRef({ session, run })
  activeBinding.current = { session, run }
  useEffect(() => { mounted.current = true; return () => {
    mounted.current = false
    const current = activeBinding.current
    const id = inFlightRun.current ?? (current.run && active.has(current.run.status) ? current.run.id : undefined)
    if (id) blockedRuns.current.add(id)
    if (current.session && id) void apiRequest(`/studio-builder/sessions/${current.session.id}/runs/${id}/cancel`, {method:'POST', keepalive:true}).catch(() => undefined)
  } }, [])
  const latest = useRef({ serverRevision, persistRecovery, checkDocument, manifests })
  latest.current = { serverRevision, persistRecovery, checkDocument, manifests }
  const setting = useQuery({ queryKey: ['studio-builder-model-setting', auth.user?.companyId, auth.user?.id], queryFn: () => apiRequest<{ modelAliasId: string | null; modelAlias: string | null; status: string;reason?: string|null }>('/studio-builder/model-setting'), refetchInterval: 5000 })

  const snapshot = useCallback(async (): Promise<Snapshot> => {
    const state = store.getState()
    const localDocumentRevision = state.localDocumentRevision
    const document = serializeStudio(state) as Snapshot['document']
    const hash = await documentHash(document)
    if (store.getState().localDocumentRevision !== localDocumentRevision) return snapshot()
    return { editorInstanceId: instance.current, localDocumentRevision, documentHash: hash, serverRevision: latest.current.serverRevision, saveStatus: state.dirty ? 'unsaved' : 'saved', document, selectedNodeIds: state.nodes.filter(node => node.selected || node.id === state.selectedId).map(node => node.id), selectedExecutionId: selectedExecution.current }
  }, [store])

  useEffect(() => {
    let alive = true
    if (!window.name.startsWith('agentx-editor-window:')) window.name = `agentx-editor-window:${crypto.randomUUID()}`
    const key = `agentx:builder-binding:${window.name}:${auth.user?.companyId}:${auth.user?.id}:${workflowId}:${draftId}`
    instance.current = sessionStorage.getItem(key) ?? crypto.randomUUID()
    sessionStorage.setItem(key, instance.current)
    void snapshot().then(value => apiRequest<Session>(`/workflows/${workflowId}/drafts/${draftId}/builder-sessions`, { method: 'POST', body: jsonBody({ snapshot: value }) })).then(async value => {
      if (!alive) return
      // Reconcile against the protocol identity, including edits made while hashing.
      const current = await snapshot()
      if (!alive) return
      store.setState(state => ({ localDocumentRevision: Math.max(state.localDocumentRevision, value.snapshot.localDocumentRevision + Number(state.localDocumentRevision !== current.localDocumentRevision || current.documentHash !== value.snapshot.documentHash)) }))
      const stored = localStorage.getItem(`agentx:builder-receipts:${value.id}`)
      proofs.current = stored ? JSON.parse(stored) as Record<string, ReceiptProof> : {}
      setSession(value)
      setRun(value.runs.find(item => active.has(item.status)))
      await apiRequest(`/studio-builder/sessions/${value.id}/snapshot`, { method: 'PUT', body: jsonBody(await snapshot()) })
    }).catch((error: Error) => alive && setError(error.message))
    return () => { alive = false }
  }, [auth.user?.companyId, auth.user?.id, draftId, snapshot, store, workflowId])

  const process = useCallback(async (entry: Entry) => {
    const payload = entry.payload
    if (payload.type === 'model_delta') setLiveText(current => current + (payload.text ?? ''))
    if (payload.type === 'message_added' && payload.message?.role === 'assistant') setLiveText('')
    setEntries(current => current.some(item => item.sequence === entry.sequence) ? current : [...current, entry].sort((a, b) => a.sequence - b.sequence))
    if (payload.type === 'terminal') setRun(current => current?.id === entry.runId ? { ...current, status: payload.status ?? 'failed' } : current)
    if (payload.type !== 'command' || !payload.command || !session || !mounted.current || blockedRuns.current.has(payload.command.runId)) return
    const command = payload.command
    if (command.draftId !== draftId || command.editorInstanceId !== instance.current) return
    const currentRun = await apiRequest<Session>(`/studio-builder/sessions/${session.id}`)
    if (!mounted.current || blockedRuns.current.has(command.runId) || !currentRun.canEdit || !currentRun.runs.some(item => item.id === command.runId && active.has(item.status))) return
    const receipt=await apiRequest<{status:string}>(`/studio-builder/sessions/${session.id}/commands/${command.commandId}/receipt`)
    if (receipt.status !== 'pending') return
    let ack = receipts.current[command.commandId]
    if (!ack && proofs.current[command.commandId]) ack = restoreBuilderReceipt(proofs.current[command.commandId]!, command.document)
    if (!ack) {
      const before = await snapshot()
      if (!mounted.current || blockedRuns.current.has(command.runId)) return
      const alreadyApplied = store.getState().appliedCommands[command.commandId] === command.expectedLocalDocumentRevision + 1
      if (alreadyApplied) {
        // Recovery can survive a crash just before the separate receipt write. Use the
        // actual recovered document only while it still proves this exact transaction.
        // Never recompute layout and claim that a different geometry was applied.
        const proven = before.localDocumentRevision === command.expectedLocalDocumentRevision + 1
          && canonicalDocument(before.document.definition) === canonicalDocument(command.document.definition)
          && canonicalDocument({ ...before.document.editorDocument, nodeLayouts: command.document.editorDocument.nodeLayouts, boundaryLayouts: command.document.editorDocument.boundaryLayouts }) === canonicalDocument(command.document.editorDocument)
          && (command.autoLayout || before.documentHash === command.resultDocumentHash)
        ack = { commandId: command.commandId, status: proven ? 'applied' : 'stale', snapshot: before,
          issues: proven ? [] : ['The applied transaction has no durable receipt and the canvas has changed. Read the current document.'] }
      }
      let status: Ack['status'] = 'stale'
      let issues: string[] = []
      if (!ack && before.localDocumentRevision === command.expectedLocalDocumentRevision && before.documentHash === command.expectedDocumentHash) {
        if (store.getState().readOnly) status = 'read_only'
        else {
          try {
            let candidate = deserializeDraft({ id: draftId, workflowId, versionId: '', versionNumber: 0, publicationStatus: 'unpublished', archived: false, generation: 0, baseVersionId: null, revision: before.serverRevision, schemaVersion: '8.0', definitionHash: '', editorHash: '', updatedAt: '', ...command.document })
            if (await documentHash(command.document) !== command.resultDocumentHash) throw new Error('Command hash does not match its document')
            if (canonicalDocument(serializeStudio(candidate)) !== canonicalDocument(command.document)) throw new Error('Document cannot be represented without changes by this editor. Include layouts, contiguous connection orders and canonical node settings.')
            if (command.autoLayout) candidate = await layoutStudioDocument(candidate, latest.current.manifests)
            issues = latest.current.checkDocument(candidate)
            if (issues.length) status = 'rejected'
            else {
              const document = serializeStudio(candidate) as Snapshot['document']
              const hash = await documentHash(document)
              const resulting: Snapshot = { ...before, document, documentHash: hash, saveStatus: 'unsaved', selectedNodeIds: before.selectedNodeIds.filter(id=>document.definition.nodes.some(node=>node.id===id)), localDocumentRevision: command.expectedLocalDocumentRevision + 1 }
              if (!mounted.current || blockedRuns.current.has(command.runId)) return
              const applied = canonicalDocument(serializeStudio(store.getState())) === canonicalDocument(before.document)
                ? store.getState().applyEditorCommand(command.commandId, before.localDocumentRevision, candidate) : 'stale'
              status = applied === 'applied' || applied === 'already_applied' ? 'applied' : applied === 'read_only' ? 'read_only' : 'stale'
              // The whole edit, including layout, is one durable Undo transaction before ACK.
              if (status === 'applied') {
                if (!latest.current.persistRecovery()) throw new Error(t('studio.builder.recoveryUnavailable'))
                ack = { commandId: command.commandId, status: 'applied', snapshot: resulting, issues: [] }
              }
            }
          } catch (cause) { status = 'rejected'; issues = [cause instanceof Error ? cause.message : String(cause)] }
        }
      }
      ack ??= { commandId: command.commandId, status, snapshot: await snapshot(), issues }
      receipts.current[command.commandId] = ack
      if (ack.status === 'applied') {
        const {document: _document,...metadata}=ack.snapshot
        proofs.current[command.commandId]={commandId:ack.commandId,status:ack.status,issues:ack.issues,snapshot:metadata,editorDocument:ack.snapshot.document.editorDocument}
        proofs.current=Object.fromEntries(Object.entries(proofs.current).slice(-64))
        localStorage.setItem(receiptKey,JSON.stringify(proofs.current))
      }
    }
    await apiRequest(`/studio-builder/sessions/${session.id}/acknowledgements`, { method: 'POST', body: jsonBody(ack) })
  }, [draftId, receiptKey, session, snapshot, store, workflowId, t])

  useEffect(() => {
    if (!session) return
    let alive = true
    const controller = new AbortController()
    const connect = async () => {
      while (alive) {
        try {
          const response = await apiRequestStream(`/studio-builder/sessions/${session.id}/events?after=${cursor.current}`, { signal: controller.signal })
          const reader = response.body?.getReader()
          if (!reader) throw new Error('Empty event stream')
          const decoder = new TextDecoder()
          let buffer = ''
          while (alive) {
            const item = await reader.read()
            if (item.done) break
            buffer += decoder.decode(item.value, { stream: true }).replace(/\r\n/g, '\n')
            let separator: number
            while ((separator = buffer.indexOf('\n\n')) >= 0) {
              const frame = buffer.slice(0, separator)
              buffer = buffer.slice(separator + 2)
              const data = frame.split('\n').filter(line => line.startsWith('data:')).map(line => line.slice(5).trimStart()).join('\n')
              if (!data) continue
              const entry = JSON.parse(data) as Entry
              await process(entry)
              cursor.current = Math.max(cursor.current, entry.sequence)
            }
          }
        } catch (error) { if (alive) setError((error as Error).message) }
        if (alive) await new Promise(resolve => setTimeout(resolve, 1000))
      }
    }
    void connect()
    return () => { alive = false; controller.abort() }
  }, [process, session])

  useEffect(() => {
    if (!session) return
    let timer: ReturnType<typeof setTimeout> | undefined
    const sync = () => {
      clearTimeout(timer)
      timer = setTimeout(() => { void snapshot().then(value => apiRequest(`/studio-builder/sessions/${session.id}/snapshot`, { method: 'PUT', body: jsonBody(value) })).catch((error: Error) => setError(error.message)) }, 200)
    }
    const unsubscribe = store.subscribe((next, previous) => { if (next.localDocumentRevision !== previous.localDocumentRevision || next.dirty !== previous.dirty) sync() })
    sync()
    return () => { unsubscribe(); clearTimeout(timer) }
  }, [session, snapshot, store, serverRevision])

  const send = async () => {
    if (!session || !prompt.trim() || busy) return
    setBusy(true); setError(undefined)
    const runId = crypto.randomUUID()
    inFlightRun.current = runId
    try {
      const current = await snapshot()
      const value = await apiRequest<{ id: string; status: string }>(`/studio-builder/sessions/${session.id}/runs`, { method: 'POST', body: jsonBody({ runId, prompt, snapshot: current, editScope: scope === 'nodes' ? {kind:'nodes',nodeIds:current.selectedNodeIds} : {kind:'canvas'} }) })
      if (!mounted.current || blockedRuns.current.has(runId)) { await apiRequest(`/studio-builder/sessions/${session.id}/runs/${runId}/cancel`, {method:'POST'}); return }
      setRun(value); setPrompt('')
    } catch (error) { if (mounted.current) setError((error as Error).message) } finally { inFlightRun.current = undefined; if (mounted.current) setBusy(false) }
  }
  const stop = async () => {
    if (session && run && active.has(run.status)) { blockedRuns.current.add(run.id); await apiRequest(`/studio-builder/sessions/${session.id}/runs/${run.id}/cancel`, { method: 'POST' }); setRun({ ...run, status: 'cancelled' }) }
  }
  const answer = async (questionId: string, answer: string) => {
    if (!session) return
    try { await apiRequest(`/studio-builder/sessions/${session.id}/answers`, { method: 'POST', body: jsonBody({ questionId, editorInstanceId: instance.current, answer }) }) } catch (error) { showToast((error as Error).message) }
  }
  return <aside className="flex w-[420px] shrink-0 flex-col border-l border-border bg-surface" data-testid="studio-builder-panel">
    <header className="flex items-center gap-2 border-b border-border p-3"><strong className="flex-1 text-sm">{t('studio.builder.title')}</strong><Button aria-label={t('common.close')} onClick={() => { void stop().finally(onClose) }} size="icon" variant="ghost"><X className="size-4" /></Button></header>
    <div className="space-y-1 border-b border-border px-4 py-3 text-xs text-muted-foreground"><p>{setting.data?.status === 'ready' ? t('studio.builder.model', { model: setting.data.modelAlias }) : setting.data?.reason ? t(`studio.builder.modelReasons.${setting.data.reason}`) : t(`studio.builder.${setting.data?.status === 'unavailable' ? 'unavailable' : 'unconfigured'}`)}</p><p>{t('studio.builder.staticOnly')}</p><p>{t(dirty ? 'studio.builder.pendingSave' : 'studio.builder.saved')}</p>{executionId && <Button onClick={() => { selectedExecution.current = executionId; void snapshot().then(value => session && apiRequest(`/studio-builder/sessions/${session.id}/snapshot`, {method:'PUT',body:jsonBody(value)})) }} size="sm" variant="ghost">{t('studio.builder.inspectSelected')}</Button>}</div>
    <div aria-live="polite" className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4 text-xs">
      {entries.map(entry => <EntryView currentHash={currentHash} answered={entries.some(item => item.payload.type === 'answer' && item.payload.questionId === entry.payload.questionId)} answer={answer} onLocate={onLocate} entry={entry} key={entry.sequence} />)}
      {liveText && <MarkdownContent>{liveText}</MarkdownContent>}
      {run && active.has(run.status) && <p className="text-muted-foreground">{t('studio.builder.working')}</p>}
      {error && <p className="text-danger" role="alert">{error}</p>}
    </div>
    <form className="space-y-2 border-t border-border p-3" onSubmit={event => { event.preventDefault(); void send() }}>
      <Select aria-label={t('studio.builder.scope')} disabled={Boolean(run && active.has(run.status))} value={scope} onValueChange={setScope} options={[{value:'canvas',label:t('studio.builder.canvasScope')},{value:'nodes',label:t('studio.builder.nodesScope'),disabled:!selection}]} />
      <textarea aria-label={t('studio.builder.prompt')} className="min-h-24 w-full resize-none rounded-md border border-input bg-surface p-3 text-xs outline-none focus:ring-2 focus:ring-ring" value={prompt} onChange={event => setPrompt(event.target.value)} placeholder={t('studio.builder.placeholder')} />
      <div className="flex justify-end gap-2"><Button disabled={!run || !active.has(run.status)} onClick={() => void stop()} size="sm" type="button" variant="secondary"><Square className="size-3" />{t('studio.builder.stop')}</Button><Button disabled={!session || setting.data?.status !== 'ready' || busy || Boolean(run && active.has(run.status)) || !prompt.trim() || (scope === 'nodes' && !selection)} size="sm" type="submit"><Send className="size-3" />{t('studio.builder.send')}</Button></div>
    </form>
  </aside>
}

function EntryView({entry,answer,onLocate,answered,currentHash}:{entry:Entry;answered:boolean;currentHash:string;onLocate:(issue:{code:string;message:string;nodeId?:string;fieldPath?:string})=>void;answer:(id:string,text:string)=>Promise<void>}) {
  const {t}=useTranslation()
  const [reply,setReply]=useState('')
  const payload=entry.payload
  if (payload.type==='user') return <p className="whitespace-pre-wrap rounded-md bg-muted p-3">{payload.text}</p>
  if (payload.type==='message_added'&&payload.message?.role==='assistant') return <MarkdownContent>{payload.message.content}</MarkdownContent>
  if (payload.type==='command'&&payload.command) return <div className="rounded-md border border-border p-3 text-muted-foreground"><p>{t('studio.builder.editBatch',{count:payload.command.changedNodeIds.length})}</p><p className="mt-1 truncate">{payload.command.changedNodeIds.map(id=>payload.command?.document.definition.nodes.find(node=>node.id===id)?.name??id).join(' · ')}</p></div>
  if (payload.type==='command_acknowledged') return <p className="rounded-md border border-border p-3">{t(`studio.builder.ack.${payload.status}`)}{payload.issues?.map(issue=><span className="block text-danger" key={issue}>{issue}</span>)}</p>
  if (payload.type==='answer') return <p className="rounded-md bg-muted p-3">{payload.text}</p>
  if (payload.type==='question'&&payload.questionId) return <div className="space-y-2 rounded-md border border-border p-3"><p>{payload.question}</p>{payload.options?.map(option=><Button disabled={answered} key={option} onClick={()=>void answer(payload.questionId!,option)} size="sm" variant="secondary">{option}</Button>)}<input disabled={answered} aria-label={t('studio.builder.answer')} className="w-full rounded border border-input bg-surface p-2" value={reply} onChange={event=>setReply(event.target.value)} /><Button disabled={answered || !reply.trim()} onClick={()=>void answer(payload.questionId!,reply)} size="sm">{t('studio.builder.answer')}</Button></div>
  if (payload.type==='validation' && payload.report) { const stale=payload.report.documentHash!==currentHash; return <div className="space-y-2 rounded-md border border-border p-3"><p>{t(stale ? 'studio.builder.staticStale' : payload.report.issues.length ? 'studio.builder.staticIssues' : 'studio.builder.staticPassed')}</p>{payload.report.issues.map((issue,index)=><Button disabled={stale} key={index} size="sm" variant="ghost" onClick={()=>onLocate({code:issue.code,message:issue.message,nodeId:issue.nodeId??undefined,fieldPath:issue.fieldPath??undefined})}>{issue.message}</Button>)}</div> }
  if (payload.type==='progress') return <p className="text-muted-foreground">{payload.text}</p>
  if (payload.type==='usage') return <p className="text-[10px] text-muted-foreground">{t('studio.builder.usage',{input:payload.inputTokens,output:payload.outputTokens,cost:(payload.costMicros??0)/1_000_000,currency:payload.currency})}</p>
  if (payload.type==='terminal') return <p className="text-muted-foreground">{t(`studio.builder.status.${payload.status ?? 'failed'}`)}</p>
  return null
}
