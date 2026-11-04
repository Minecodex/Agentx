import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { Button } from '../ui/button'
import { Input } from '../ui/input'
import { MarkdownContent } from '../ui/markdown-content'

export type DesignEntryPayload = { type: string; text?: string; messageId?:string; inputKind?:'steering'|'follow_up'; status?: string; code?:string; questionId?: string; question?: string; options?: string[]; message?: {role:string;content:string;isError:boolean}; inputTokens?:number;outputTokens?:number;costMicros?:number;currency?:string }

/** Shared authoring conversation: business task messages and approvals never enter this component. */
export function DesignAgentEntry({ payload, answered, answer, currentHash }: { payload: DesignEntryPayload; answered: boolean; answer: (id: string, text: string) => Promise<void>; currentHash?:string }) {
  const { t } = useTranslation()
  const [reply, setReply] = useState('')
  if (payload.type === 'user' || payload.type === 'answer') return <p data-design-input-id={payload.inputKind?payload.messageId:undefined} className="whitespace-pre-wrap rounded-md bg-muted p-3">{payload.inputKind&&<span className="mb-1 block text-[10px] text-muted-foreground">{t('studio.builder.guidance')}</span>}{payload.text}</p>
  if (payload.type === 'message_added' && payload.message?.role === 'assistant') return <MarkdownContent>{payload.message.content}</MarkdownContent>
  if (payload.type === 'message_added' && payload.message?.role === 'tool_result') {
    let value: {source?:string;documentHash?:string;image?:{width:number;height:number};report?:{errors?:string[];actions?:{status?:string;check?:{errors?:{message:string}[]}}[]}}|undefined
    try { value=JSON.parse(payload.message.content) } catch { return null }
    if(value?.source!=='isolated_browser'||!value.report)return null
    const stale=Boolean(currentHash&&currentHash!==value.documentHash)
    const issues=[...(value.report.errors??[]),...(value.report.actions??[]).flatMap(action=>action.status==='failed'?[t('studio.builder.browserActionFailed')]:(action.check?.errors??[]).map(issue=>issue.message))]
    return <div data-testid="design-browser-observation" className="space-y-1 rounded-md border border-border p-3"><p className={stale?'text-muted-foreground':issues.length?'text-danger':'text-foreground'}>{t(stale?'studio.builder.browserStale':issues.length?'studio.builder.browserIssues':'studio.builder.browserObserved')}</p><p className="text-muted-foreground">{t(value.image?'studio.builder.browserImage':'studio.builder.browserDom',{width:value.image?.width,height:value.image?.height})}</p><p className="text-muted-foreground">{t('studio.builder.browserSynthetic')}</p>{issues.map((issue,index)=><p className="text-danger" key={index}>{issue}</p>)}</div>
  }
  if (payload.type === 'question' && payload.questionId) return <div className="space-y-2 rounded-md border border-border p-3">
    <p>{payload.question}</p><div className="flex flex-wrap gap-2">{payload.options?.map(option => <Button disabled={answered} key={option} onClick={() => void answer(payload.questionId!, option)} size="sm" variant="secondary">{option}</Button>)}</div>
    <Input disabled={answered} aria-label={t('studio.builder.answer')} value={reply} onChange={event => setReply(event.target.value)} />
    <Button disabled={answered || !reply.trim()} onClick={() => void answer(payload.questionId!, reply)} size="sm">{t('studio.builder.answer')}</Button>
  </div>
  if (payload.type === 'progress') return <p className="text-muted-foreground">{payload.text}</p>
  if (payload.type === 'model_failed') return <p className="text-danger" role="alert">{t(payload.code==='DESIGN_MODEL_OUTPUT_LIMIT'?'studio.builder.outputLimit':'studio.builder.modelFailed')} · {payload.code}</p>
  if (payload.type === 'usage') return <p className="text-[10px] text-muted-foreground">{t('studio.builder.usage', { input: payload.inputTokens, output: payload.outputTokens, cost: (payload.costMicros ?? 0) / 1_000_000, currency: payload.currency })}</p>
  if (payload.type === 'terminal') return <p className="text-muted-foreground">{t(`studio.builder.status.${payload.status ?? 'failed'}`)}</p>
  return null
}
