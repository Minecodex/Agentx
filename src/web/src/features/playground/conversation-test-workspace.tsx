import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Bot, ExternalLink, File, MessageSquarePlus, Plus, Send, Settings2, Square, UserRound } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, gatewayRequest, jsonBody } from '../../shared/api/client'
import type { Application, ApplicationSession, GatewayInvocation, GatewayMessage, GatewaySession } from '../../shared/api/types'
import type { ArtifactReference } from '../../shared/components/schema-form'
import { localizedValue } from '../../shared/lib/localized-value'
import { Badge } from '../../shared/ui/badge'
import { Button } from '../../shared/ui/button'
import { Textarea } from '../../shared/ui/textarea'
import { Tooltip } from '../../shared/ui/tooltip'
import { useToast } from '../../shared/ui/toast'
import { useExecutionArtifactDownload } from '../traces/use-execution-artifact-download'
import { ChatMappingDialog } from './chat-mapping-dialog'
import type { ChatMapping, PlaygroundConfig, PlaygroundDeployment } from './playground-types'
import { useInvocationEvents } from './use-invocation-events'

const terminal = new Set(['completed', 'failed', 'cancelled'])

export function ConversationTestWorkspace({ application, deployment, sessionId, onSessionChange, uploadArtifact }: {
  application: Application
  deployment: PlaygroundDeployment
  sessionId: string
  onSessionChange: (sessionId: string) => void
  uploadArtifact: (file: File) => Promise<ArtifactReference>
}) {
  const auth = useAuth()
  const { t, i18n } = useTranslation()
  const { showToast } = useToast()
  const queryClient = useQueryClient()
  const fileInput = useRef<HTMLInputElement>(null)
  const [createdSession, setCreatedSession] = useState<GatewaySession | null>(null)
  const [message, setMessage] = useState('')
  const [files, setFiles] = useState<ArtifactReference[]>([])
  const [uploading, setUploading] = useState(false)
  const [mappingOpen, setMappingOpen] = useState(false)
  const [invocationId, setInvocationId] = useState<string>()
  const [deltaBuffer, setDeltaBuffer] = useState('')
  const sessions = useQuery({ queryKey: ['application-sessions', application.id], queryFn: () => apiRequest<ApplicationSession[]>(`/applications/${application.id}/sessions`) })
  const config = useQuery({ queryKey: ['playground-config', application.id, deployment.id], queryFn: () => apiRequest<PlaygroundConfig>(`/applications/${application.id}/deployments/${deployment.id}/playground-config`), refetchInterval: (query) => query.state.data?.publishStatus === 'publishing' ? 1_000 : false })
  const sessionList: Array<ApplicationSession | GatewaySession> = createdSession && !sessions.data?.some((item) => item.id === createdSession.id) ? [createdSession, ...(sessions.data ?? [])] : (sessions.data ?? [])
  const session = sessionList.find((item) => item.id === sessionId)
  const messages = useQuery({ queryKey: ['gateway-messages', sessionId], queryFn: () => gatewayRequest<GatewayMessage[]>(`/sessions/${sessionId}/messages`), enabled: Boolean(sessionId) })
  const invocation = useQuery({ queryKey: ['gateway-invocation', invocationId], queryFn: () => gatewayRequest<GatewayInvocation>(`/invocations/${invocationId}`), enabled: Boolean(invocationId), refetchInterval: (query) => query.state.data && terminal.has(query.state.data.status) ? false : 2_000 })
  const running = Boolean(invocation.data && !terminal.has(invocation.data.status))
  useInvocationEvents(invocationId, running, (eventType, payload) => {
    if (eventType === 'model.delta') {
      const delta = (payload as { deltaText?: unknown } | null)?.deltaText
      if (typeof delta === 'string' && delta) setDeltaBuffer((current) => current + delta)
      return
    }
    void queryClient.invalidateQueries({ queryKey: ['gateway-invocation', invocationId] })
    if (eventType.startsWith('invocation.')) void queryClient.invalidateQueries({ queryKey: ['gateway-messages', sessionId] })
  })
  useEffect(() => {
    if (invocation.data && terminal.has(invocation.data.status)) {
      void queryClient.invalidateQueries({ queryKey: ['gateway-messages', sessionId] })
    }
  }, [invocation.data, queryClient, sessionId])
  useEffect(() => {
    if (invocation.data && terminal.has(invocation.data.status) && deltaBuffer) setDeltaBuffer('')
  }, [deltaBuffer, invocation.data])
  const createSession = useMutation({
    mutationFn: () => gatewayRequest<GatewaySession>(`/applications/${application.slug}/sessions`, { method: 'POST', headers: { 'Idempotency-Key': crypto.randomUUID() }, body: jsonBody({ title: null, externalUserId: null }) }),
    onSuccess: async (value) => { setCreatedSession(value); setInvocationId(undefined); onSessionChange(value.id); await queryClient.invalidateQueries({ queryKey: ['application-sessions', application.id] }) },
    onError: (error: Error) => showToast(error.message),
  })
  const saveMapping = useMutation({ mutationFn: (mapping: ChatMapping | null) => apiRequest<PlaygroundConfig>(`/applications/${application.id}/deployments/${deployment.id}/playground-config`, { method: 'PUT', body: jsonBody({ expectedVersion: config.data?.version ?? 0, mapping }) }), onSuccess: (value) => { queryClient.setQueryData(['playground-config', application.id, deployment.id], value); setMappingOpen(false) }, onError: (error: Error) => showToast(error.message) })
  const send = useMutation({
    mutationFn: () => gatewayRequest<GatewayInvocation>(`/sessions/${sessionId}/messages`, { method: 'POST', headers: { 'Idempotency-Key': crypto.randomUUID() }, body: jsonBody({ parts: [{ partType: 'text', content: message }, ...files.map((file) => ({ partType: 'file', content: file, artifactId: file.artifactId }))] }) }),
    onSuccess: async (value) => {
      setInvocationId(value.id)
      setMessage('')
      setFiles([])
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['gateway-messages', sessionId] }),
        queryClient.invalidateQueries({ queryKey: ['application-sessions', application.id] }),
      ])
    },
    onError: (error: Error) => showToast(error.message),
  })
  const cancel = useMutation({ mutationFn: () => gatewayRequest(`/invocations/${invocationId}/cancel`, { method: 'POST', headers: { 'Idempotency-Key': crypto.randomUUID() } }), onSuccess: () => queryClient.invalidateQueries({ queryKey: ['gateway-invocation', invocationId] }) })
  const activeMapping = config.data?.publishStatus === 'active' ? config.data.mapping : null
  const canSend = Boolean(session?.status === 'active' && activeMapping && !running && !uploading)
  const chooseFiles = async (selected: FileList | null) => {
    if (!selected?.length) return
    setUploading(true)
    try {
      const uploaded = await Promise.all(Array.from(selected).map(uploadArtifact))
      setFiles((current) => [...current, ...uploaded])
    } catch (error) {
      showToast(error instanceof Error ? error.message : String(error))
    } finally {
      setUploading(false)
    }
  }
  const requestSend = () => { if (message.trim() && canSend) send.mutate() }
  return <div className="grid h-full min-h-0 grid-cols-[280px_minmax(0,1fr)] overflow-hidden rounded-lg border border-border bg-surface">
    <aside className="flex min-h-0 flex-col border-r border-border bg-muted/20">
      <div className="flex h-14 shrink-0 items-center justify-between border-b border-border px-4"><strong className="text-sm">{t('applications.playground.conversationHistory')}</strong><Button aria-label={t('applications.playground.newSession')} disabled={createSession.isPending} onClick={() => createSession.mutate()} size="icon" variant="ghost"><MessageSquarePlus className="size-4" /></Button></div>
      <div className="min-h-0 flex-1 space-y-1 overflow-auto p-2">{sessions.isLoading ? <Hint>{t('common.loading')}</Hint> : sessionList.length ? sessionList.map((item) => {
        const title = item.title ?? t('applications.playground.sessionTitle')
        return <button aria-pressed={item.id === sessionId} className={`w-full rounded-md p-3 text-left ${item.id === sessionId ? 'bg-primary/10' : 'hover:bg-muted'}`} key={item.id} onClick={() => { setInvocationId(undefined); onSessionChange(item.id) }} type="button"><span className="flex items-center gap-2"><Tooltip content={title}><strong className="min-w-0 flex-1 truncate text-xs">{title}</strong></Tooltip><Badge tone={item.status === 'active' ? 'success' : 'neutral'}>{localizedValue(t, 'common', item.status)}</Badge></span><span className="mt-1 block truncate font-mono text-[9px] text-muted-foreground">{item.id}</span></button>
      }) : <Hint>{t('applications.playground.noSessions')}</Hint>}</div>
    </aside>
    <section className="flex min-h-0 min-w-0 flex-col">
      <div className="flex h-14 shrink-0 items-center gap-3 border-b border-border px-5"><span className="grid size-8 place-items-center rounded-md bg-primary/10 text-primary"><Bot className="size-4" /></span><div><strong className="block text-xs">{application.name}</strong><span className="text-[10px] text-muted-foreground">{config.data?.publishStatus === 'publishing' ? t('applications.playground.mappingPublishing') : config.data?.mapping ? t('applications.playground.mappingVersion', { version: config.data.publishedVersion ?? config.data.version }) : t('applications.playground.mappingMissing')}</span></div><Tooltip content={t('applications.playground.mappingButton')}><Button aria-label={t('applications.playground.mappingButton')} className="ml-auto" onClick={() => setMappingOpen(true)} size="icon" variant="secondary"><Settings2 className="size-4" /></Button></Tooltip>{invocation.data && <Badge tone={invocation.data.status === 'failed' ? 'danger' : running ? 'warning' : 'success'}>{localizedValue(t, 'common', invocation.data.status)}</Badge>}{running && <Button aria-label={t('applications.playground.stop')} onClick={() => cancel.mutate()} size="icon" variant="ghost"><Square className="size-4" /></Button>}</div>
      {config.data?.publishStatus === 'failed' && <div className="border-b border-danger/20 bg-danger/10 px-5 py-2 text-xs text-danger">{config.data.errorCode}: {config.data.errorMessage}</div>}
      <div className="min-h-0 flex-1 overflow-y-auto px-6 py-5">{!session ? <Empty icon={<UserRound className="size-6" />} text={t('applications.playground.selectSessionHint')} /> : messages.isLoading ? <Hint>{t('common.loading')}</Hint> : !messages.data?.length ? <Empty icon={<Bot className="size-6" />} text={t('applications.playground.firstMessageHint')} /> : <div className="space-y-4">{messages.data.map((item) => <MessageBubble key={item.id} message={item} locale={i18n.resolvedLanguage} />)}{running && deltaBuffer && <div className="flex justify-start"><div className="max-w-[78%] whitespace-pre-wrap break-words rounded-md border border-border bg-muted/40 px-4 py-3 text-xs leading-5">{deltaBuffer}<span className="ml-0.5 inline-block h-3 w-1.5 animate-pulse bg-primary align-middle" /></div></div>}</div>}</div>
      <div className="shrink-0 border-t border-border p-4">
        <div className="relative rounded-lg border border-border bg-background transition-shadow focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15">
          {files.length > 0 && <div className="flex flex-wrap gap-2 px-3 pt-3">{files.map((file) => <span className="flex items-center gap-1 rounded-md bg-muted px-2 py-1 text-[10px]" key={file.artifactId}><File className="size-3" />{file.fileName ?? file.artifactId}<button aria-label={t('applications.playground.removeAttachment')} onClick={() => setFiles((items) => items.filter((item) => item.artifactId !== file.artifactId))} type="button">×</button></span>)}</div>}
          <Textarea className="max-h-52 min-h-28 resize-none border-0 bg-transparent pb-12 shadow-none focus:border-transparent focus:ring-0" disabled={!session || running || uploading} onChange={(event) => setMessage(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); requestSend() } }} placeholder={activeMapping ? t('applications.playground.composer') : t('applications.playground.composerNeedsMapping')} value={message} />
          <div className="pointer-events-none absolute inset-x-2 bottom-2 flex items-center justify-between"><input className="sr-only" multiple onChange={(event) => void chooseFiles(event.target.files)} ref={fileInput} type="file" /><Button aria-label={t('applications.playground.addAttachment')} className="pointer-events-auto" disabled={!session || !activeMapping?.fileInput || running || uploading} onClick={() => fileInput.current?.click()} size="icon" variant="ghost"><Plus className="size-4" /></Button><Button aria-label={t('applications.playground.send')} className="pointer-events-auto" disabled={!activeMapping || !canSend || !message.trim() || send.isPending} onClick={requestSend} size="icon"><Send className="size-4" /></Button></div>
        </div>
      </div>
    </section>
    <ChatMappingDialog canManage={auth.hasPermission('application:manage')} config={config.data} inputSchema={deployment.inputSchema} onClear={() => saveMapping.mutate(null)} onOpenChange={setMappingOpen} onSave={(mapping) => saveMapping.mutate(mapping)} open={mappingOpen} outputSchema={deployment.outputSchema} saving={saveMapping.isPending} />
  </div>
}

function MessageBubble({ message, locale }: { message: GatewayMessage; locale?: string }) {
  const { t } = useTranslation()
  const invocation = useQuery({ queryKey: ['gateway-invocation', 'message', message.invocationId], queryFn: () => gatewayRequest<GatewayInvocation>(`/invocations/${message.invocationId}`), enabled: message.role === 'assistant' && Boolean(message.invocationId), staleTime: Infinity })
  const download = useExecutionArtifactDownload(invocation.data?.executionId ?? undefined)
  const user = message.role === 'user'
  return <article className={`flex gap-3 ${user ? 'justify-end' : 'justify-start'}`}><div className={`max-w-[78%] rounded-md px-4 py-3 text-xs leading-5 ${user ? 'bg-primary text-primary-foreground' : 'border border-border bg-muted/40'}`}>{message.parts.map((part, index) => part.artifactId ? <button className={`my-1 flex w-full items-center gap-2 rounded border p-2 text-left ${user ? 'border-primary-foreground/30' : 'border-border bg-surface'}`} key={`${part.artifactId}-${index}`} onClick={() => void download(part.artifactId ?? '')} type="button"><File className="size-4" /><span className="truncate">{fileLabel(part.content, part.artifactId)}</span></button> : <div className="whitespace-pre-wrap break-words" key={`${part.partType}-${index}`}>{typeof part.content === 'string' ? part.content : JSON.stringify(part.content, null, 2)}</div>)}<div className="mt-2 flex items-center gap-2 text-[10px] opacity-70"><span>{new Date(message.createdAt).toLocaleString(locale)}</span>{!user && invocation.data?.executionId && <Link className="inline-flex items-center gap-1 hover:underline" to={`/executions/${invocation.data.executionId}`}><ExternalLink className="size-3" />{t('applications.playground.messageExecutionTrace')}</Link>}</div></div></article>
}

function fileLabel(content: unknown, fallback: string) { return content && typeof content === 'object' && !Array.isArray(content) && typeof (content as Record<string, unknown>).fileName === 'string' ? String((content as Record<string, unknown>).fileName) : fallback }
function Hint({ children }: { children: React.ReactNode }) { return <p className="p-4 text-center text-xs text-muted-foreground">{children}</p> }
function Empty({ icon, text }: { icon: React.ReactNode; text: string }) { return <div className="flex h-full min-h-80 flex-col items-center justify-center text-center text-muted-foreground"><span className="grid size-12 place-items-center rounded-md bg-muted">{icon}</span><strong className="mt-3 text-xs font-medium text-foreground">{text}</strong></div> }
