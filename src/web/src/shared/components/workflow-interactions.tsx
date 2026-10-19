import { useTranslation } from 'react-i18next'

type Message = { id: string; content: string; createdAt?: string }
type Wait = { waitId: string; nodePath: string; occurrence: number; status: string; content?: unknown; source?: string | null }

export function WorkflowInteractions({ messages = [], waits = [] }: { messages?: readonly Message[]; waits?: readonly Wait[] }) {
  const { t } = useTranslation()
  const sources: Record<string,string> = { preset:t('interaction.presetSource'), api:t('interaction.apiSource'), channel:t('interaction.channelSource'), platform:t('interaction.platformSource'), timeout:t('interaction.timedOut') }
  const statuses: Record<string,string> = { waiting:t('common.waiting'), received:t('interaction.received'), timed_out:t('interaction.timedOut'), cancelled:t('common.cancelled'), failed:t('common.failed') }
  if (!messages.length && !waits.length) return null
  return <div className="space-y-4" aria-live="polite" data-testid="workflow-interactions">
    {messages.length > 0 && <section><h3 className="mb-2 text-xs font-semibold">{t('interaction.messages')}</h3><div className="space-y-2">{messages.map(message => <p className="whitespace-pre-wrap break-words rounded-md border border-border bg-muted/20 p-3 text-xs" key={message.id}>{message.content}</p>)}</div></section>}
    {waits.length > 0 && <section><h3 className="mb-2 text-xs font-semibold">{t('interaction.waits')}</h3><div className="space-y-2">{waits.map(wait => <div key={wait.waitId} className="space-y-1 rounded-md border border-border p-3 text-xs"><div className="flex flex-wrap items-center gap-2"><code>{wait.nodePath}</code><span className="text-muted-foreground">{t('interaction.occurrence', { number: wait.occurrence })}</span><span>{statuses[wait.status] ?? wait.status}</span></div>{wait.source && <p className="text-muted-foreground">{t('interaction.source')}: {sources[wait.source] ?? wait.source}</p>}{wait.content !== undefined && <pre className="whitespace-pre-wrap break-all font-mono text-[11px]">{typeof wait.content === 'string' ? wait.content : JSON.stringify(wait.content, null, 2)}</pre>}</div>)}</div></section>}
  </div>
}
