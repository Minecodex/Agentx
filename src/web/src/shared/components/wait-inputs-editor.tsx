import { useEffect, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Plus, Trash2 } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { apiRequest } from '../api/client'
import { Button } from '../ui/button'
import { Select } from '../ui/select'
import { Textarea } from '../ui/textarea'

export type WaitInputNode = { path: string; nodeId: string; name: string }
export type WaitInputs = Record<string, unknown[]>

export function useApplicationWaitNodes(applicationId: string, versionKey?: string, workflowVersionId?: string) {
  return useQuery({ queryKey: ['application-wait-nodes', applicationId, versionKey, workflowVersionId], queryFn: () => apiRequest<WaitInputNode[]>(`/applications/${applicationId}/wait-input-nodes${workflowVersionId ? `?workflowVersionId=${encodeURIComponent(workflowVersionId)}` : ''}`), enabled: Boolean(applicationId) })
}

export function WaitInputsEditor({ nodes, value, onChange, disabled = false, onValidityChange }: {
  nodes: readonly WaitInputNode[]; value: WaitInputs; onChange: (value: WaitInputs) => void; disabled?: boolean; onValidityChange?: (valid: boolean) => void
}) {
  const { t } = useTranslation()
  const [invalid, setInvalid] = useState<Set<string>>(new Set())
  const rowIds = useRef<Record<string, string[]>>({})
  useEffect(() => { onValidityChange?.(invalid.size === 0) }, [invalid, onValidityChange])
  useEffect(() => {
    const active = new Set(nodes.flatMap(node => rowIds.current[node.path]?.slice(0, value[node.path]?.length ?? 0) ?? []))
    setInvalid(current => { const next = new Set([...current].filter(id => active.has(id))); return next.size === current.size ? current : next })
  }, [nodes, value])
  const markValid = (id: string, valid: boolean) => setInvalid(current => { if (current.has(id) === !valid) return current; const next = new Set(current); if (valid) next.delete(id); else next.add(id); return next })
  for (const node of nodes) { const ids = rowIds.current[node.path] ??= []; const count = value[node.path]?.length ?? 0; if (ids.length > count) ids.length = count; while (ids.length < count) ids.push(crypto.randomUUID()); }
  if (!nodes.length) return null
  const update = (path: string, replies: unknown[]) => onChange({ ...value, [path]: replies })
  return <section className="space-y-3" data-testid="wait-inputs-editor">
    <div><h3 className="text-xs font-semibold">{t('interaction.presets')}</h3><p className="mt-1 text-xs leading-5 text-muted-foreground">{t('interaction.presetsHint')}</p></div>
    {nodes.map(node => <div key={node.path} className="space-y-2 rounded-lg border border-border p-3" data-node-path={node.path}>
      <div className="flex items-center justify-between gap-3"><div className="min-w-0"><strong className="text-xs">{node.name}</strong><code className="mt-1 block break-all text-[10px] text-muted-foreground">{node.path}</code></div><Button type="button" size="sm" variant="ghost" disabled={disabled} onClick={() => update(node.path, [...(value[node.path] ?? []), ''])}><Plus className="size-3" />{t('interaction.addReply')}</Button></div>
      {(value[node.path] ?? []).map((reply, index) => <div key={rowIds.current[node.path][index]} className="space-y-1"><div className="flex items-center justify-between"><span className="text-[10px] text-muted-foreground">{t('interaction.replyNumber', { number: index + 1 })}</span><Button type="button" size="icon" variant="ghost" aria-label={t('interaction.removeReply')} disabled={disabled} onClick={() => { markValid(rowIds.current[node.path][index], true); rowIds.current[node.path].splice(index, 1); update(node.path, (value[node.path] ?? []).filter((_, position) => position !== index)); }}><Trash2 className="size-3" /></Button></div><ReplyValue onValidityChange={valid => markValid(rowIds.current[node.path][index], valid)} value={reply} disabled={disabled} onChange={next => update(node.path, (value[node.path] ?? []).map((current, position) => position === index ? next : current))} /></div>)}
      {!value[node.path]?.length && <p className="text-xs text-warning">{t('interaction.missingReply')}</p>}
    </div>)}
  </section>
}

function ReplyValue({ value, onChange, disabled, onValidityChange }: { value: unknown; onChange: (value: unknown) => void; disabled: boolean; onValidityChange: (valid: boolean) => void }) {
  const { t } = useTranslation()
  const [mode, setMode] = useState(typeof value === 'string' ? 'text' : 'json')
  const [draft, setDraft] = useState<string>()
  const [error, setError] = useState(false)
  const inputRef = useRef<HTMLTextAreaElement>(null)
  useEffect(() => { inputRef.current?.setCustomValidity(error ? t('interaction.invalidJson') : '') }, [error, t])
  return <div className="space-y-2"><Select aria-label={t('interaction.contentFormat')} disabled={disabled} value={mode} options={[{ value: 'text', label: t('interaction.text') }, { value: 'json', label: 'JSON' }]} onValueChange={next => { setMode(next); setDraft(undefined); setError(false); onValidityChange(true); if (next === 'text' && typeof value !== 'string') onChange(JSON.stringify(value)); }} /><Textarea ref={inputRef} aria-invalid={error} aria-label={t('interaction.replyContent')} className="min-h-20 text-xs" disabled={disabled} value={draft ?? (mode === 'text' ? String(value ?? '') : JSON.stringify(value, null, 2))} onChange={event => {
    const raw = event.target.value
    if (mode === 'text') { onChange(raw); onValidityChange(true); return }
    setDraft(raw)
    try { onChange(JSON.parse(raw)); setError(false); onValidityChange(true) } catch { setError(true); onValidityChange(false) }
  }} />{error && <p role="alert" className="text-xs text-danger">{t('interaction.invalidJson')}</p>}</div>
}
