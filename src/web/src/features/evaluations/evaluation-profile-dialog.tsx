import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'
import { apiRequest } from '../../shared/api/client'
import type { Model, PageResponse } from '../../shared/api/types'
import { Plus, Trash2 } from 'lucide-react'
import { type FormEvent, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { Button } from '../../shared/ui/button'
import { Dialog, DialogContent } from '../../shared/ui/dialog'
import { Input } from '../../shared/ui/input'
import { Select } from '../../shared/ui/select'

export type EvaluationRuleDraft = {
  key: string
  name: string
  evaluatorType: string
  configuration: Record<string, unknown>
  weight: string
  required: boolean
}

export type EvaluationProfileDraft = {
  name: string
  description: string | null
  visibility: string
  aggregation: string
  passThreshold: string
  rules: EvaluationRuleDraft[]
}

type RuleForm = Omit<EvaluationRuleDraft, 'configuration'> & { id: string; configuration: string; judgeModelId: string; judgePrompt: string }

export function EvaluationProfileDialog({ onClose, onSubmit, open }: { onClose: () => void; onSubmit: (value: EvaluationProfileDraft) => Promise<void>; open: boolean }) {
  const { t } = useTranslation()
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [visibility, setVisibility] = useState('department')
  const [aggregation, setAggregation] = useState('all')
  const [passThreshold, setPassThreshold] = useState('1')
  const [rules, setRules] = useState<RuleForm[]>([newRule(0)])
  const [error, setError] = useState('')
  const [pending, setPending] = useState(false)
  const models = useQuery({ queryKey: ['evaluation-judge-models'], enabled: open && rules.some((rule) => rule.evaluatorType === 'llm_judge'), queryFn: () => apiRequest<PageResponse<Model>>('/models/aliases?pageSize=100&status=active') })
  const updateRule = (id: string, value: Partial<RuleForm>) => setRules((current) => current.map((rule) => rule.id === id ? { ...rule, ...value } : rule))
  const submit = async (event: FormEvent) => {
    event.preventDefault()
    setError('')
    try {
      if (!name.trim() || rules.some((rule) => !rule.name.trim() || !rule.key.trim())) throw new Error(t('evaluations.profileRequiredFields'))
      const parsed = rules.map(({ id: _, configuration, judgeModelId, judgePrompt, ...rule }) => {
        if (rule.evaluatorType === 'llm_judge') {
          if (!judgeModelId.trim() || !judgePrompt.trim()) throw new Error(t('evaluations.judgeRequiredFields'))
          if (new TextEncoder().encode(judgePrompt).byteLength > 64 * 1024) throw new Error(t('evaluations.judgePromptLimit'))
          return { ...rule, configuration: { modelId: judgeModelId.trim(), prompt: judgePrompt } }
        }
        return { ...rule, configuration: JSON.parse(configuration || '{}') as Record<string, unknown> }
      })
      setPending(true)
      await onSubmit({ name: name.trim(), description: description.trim() || null, visibility, aggregation, passThreshold, rules: parsed })
      onClose()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setPending(false)
    }
  }
  return <Dialog onOpenChange={(value) => { if (!value && !pending) onClose() }} open={open}>
    <DialogContent className="w-[min(900px,calc(100vw-48px))]" description={t('evaluations.profileDescription')} title={t('evaluations.newProfile')}>
      <form onSubmit={(event) => void submit(event)}>
        <div className="border-b border-border px-6 py-5"><h2 className="text-base font-semibold">{t('evaluations.newProfile')}</h2><p className="mt-1 text-xs text-muted-foreground">{t('evaluations.profileDescription')}</p></div>
        <div className="space-y-6 p-6">
          <div className="grid grid-cols-2 gap-4">
            <Field label={t('common.name')} required><Input onChange={(event) => setName(event.target.value)} required value={name} /></Field>
            <Field label={t('evaluations.visibility')} required><Select aria-label={t('evaluations.visibility')} aria-required className="w-full" onValueChange={setVisibility} options={[{ value: 'private', label: t('evaluations.private') }, { value: 'department', label: t('evaluations.department') }, { value: 'company', label: t('evaluations.company') }]} value={visibility} /></Field>
            <Field className="col-span-2" label={t('common.description')}><textarea className="min-h-20 w-full rounded-lg border border-border bg-surface px-3 py-2 text-sm outline-none focus:border-primary/60 focus:ring-2 focus:ring-primary/15" onChange={(event) => setDescription(event.target.value)} value={description} /></Field>
            <Field label={t('evaluations.aggregation')} required><Select aria-label={t('evaluations.aggregation')} aria-required className="w-full" onValueChange={setAggregation} options={['all', 'any', 'weighted'].map((value) => ({ value, label: t(`evaluations.aggregationOptions.${value}`) }))} value={aggregation} /></Field>
            <Field label={t('evaluations.passThreshold')} required><Input max="1" min="0" onChange={(event) => setPassThreshold(event.target.value)} required step="0.01" type="number" value={passThreshold} /></Field>
          </div>
          <section>
            <div className="flex items-center justify-between"><div><h3 className="text-sm font-semibold">{t('evaluations.scoringRules')}</h3><p className="mt-1 text-xs text-muted-foreground">{t('evaluations.scoringRulesDescription')}</p></div><Button onClick={() => setRules((current) => [...current, newRule(current.length)])} size="sm" type="button" variant="secondary"><Plus className="size-3.5" />{t('evaluations.addRule')}</Button></div>
            <div className="mt-4 space-y-3">{rules.map((rule, index) => <div className="rounded-lg border border-border bg-canvas/30 p-4" key={rule.id}>
              <div className="flex items-center justify-between"><span className="text-xs font-semibold">{t('evaluations.ruleNumber', { number: index + 1 })}</span><Button aria-label={t('evaluations.removeRule')} disabled={rules.length === 1} onClick={() => setRules((current) => current.filter((item) => item.id !== rule.id))} size="icon" type="button" variant="ghost"><Trash2 className="size-3.5" /></Button></div>
              <div className="mt-3 grid grid-cols-2 gap-3">
                <Field label={t('evaluations.ruleName')} required><Input onChange={(event) => updateRule(rule.id, { name: event.target.value })} required value={rule.name} /></Field>
                <Field label={t('evaluations.ruleKey')} required><Input onChange={(event) => updateRule(rule.id, { key: event.target.value })} pattern="[A-Za-z0-9_-]+" required value={rule.key} /></Field>
                <Field label={t('evaluations.ruleType')} required><Select aria-label={t('evaluations.ruleType')} aria-required className="w-full" onValueChange={(value) => updateRule(rule.id, { evaluatorType: value })} options={['exact', 'contains', 'regex', 'json_schema', 'llm_judge'].map((value) => ({ value, label: t(`evaluations.ruleTypes.${value}`) }))} value={rule.evaluatorType} /></Field>
                <Field label={t('evaluations.ruleWeight')} required><Input min="0.0001" onChange={(event) => updateRule(rule.id, { weight: event.target.value })} required step="any" type="number" value={rule.weight} /></Field>
                {rule.evaluatorType === 'llm_judge' ? <>
                  <Field label={t('evaluations.judgeModel')} required><Select aria-label={t('evaluations.judgeModel')} className="w-full" onValueChange={(value) => updateRule(rule.id, { judgeModelId: value })} options={(models.data?.items ?? []).map((model) => ({ value: model.id, label: model.alias }))} value={rule.judgeModelId} /></Field>
                  <Field label={t('evaluations.judgePrompt')} required><textarea className="min-h-20 w-full rounded-lg border border-border bg-canvas/30 px-3 py-2 font-mono text-xs outline-none focus:border-primary/60 focus:ring-2 focus:ring-primary/15" onChange={(event) => updateRule(rule.id, { judgePrompt: event.target.value })} placeholder="Evaluate whether the actual output matches… {{actualOutput}} {{expectedOutput}}" required value={rule.judgePrompt} /></Field>
                  <div className="col-span-2 flex flex-wrap gap-2"><Button onClick={() => updateRule(rule.id, { judgePrompt: `${rule.judgePrompt}{{actualOutput}}` })} size="sm" type="button" variant="secondary">{'{{actualOutput}}'}</Button><Button onClick={() => updateRule(rule.id, { judgePrompt: `${rule.judgePrompt}{{expectedOutput}}` })} size="sm" type="button" variant="secondary">{'{{expectedOutput}}'}</Button></div>
                  <p className="col-span-2 text-[11px] text-muted-foreground">{t('evaluations.judgeHint')} {rule.judgeModelId && <Link className="text-primary hover:underline" to={`/models/${rule.judgeModelId}`}>{t('evaluations.judgeAuthorization')}</Link>}{models.isError && <span className="text-danger">{t('common.loadFailed')}</span>}</p>
                </> : <Field className="col-span-2" label={t('evaluations.ruleConfiguration')}><textarea className="min-h-20 w-full rounded-lg border border-border bg-canvas/30 px-3 py-2 font-mono text-xs outline-none focus:border-primary/60 focus:ring-2 focus:ring-primary/15" onChange={(event) => updateRule(rule.id, { configuration: event.target.value })} value={rule.configuration} /></Field>}
                <label className="col-span-2 flex items-center gap-2 text-xs"><input checked={rule.required} onChange={(event) => updateRule(rule.id, { required: event.target.checked })} type="checkbox" />{t('evaluations.ruleRequired')}</label>
              </div>
            </div>)}</div>
          </section>
          {error && <p className="rounded-lg border border-danger/25 bg-danger/10 px-3 py-2 text-xs text-danger">{error}</p>}
        </div>
        <div className="flex justify-end gap-2 border-t border-border px-6 py-4"><Button disabled={pending} onClick={onClose} type="button" variant="ghost">{t('common.cancel')}</Button><Button disabled={pending} type="submit">{t('common.save')}</Button></div>
      </form>
    </DialogContent>
  </Dialog>
}

function newRule(index: number): RuleForm { return { id: crypto.randomUUID(), key: `rule_${index + 1}`, name: '', evaluatorType: 'exact', configuration: '{}', judgeModelId: '', judgePrompt: '', weight: '1', required: true } }
function Field({ children, className = '', label, required = false }: { children: React.ReactNode; className?: string; label: string; required?: boolean }) { return <label className={`space-y-1.5 text-xs font-medium ${className}`}><span>{label}{required && <span aria-hidden="true" className="ml-1 text-danger">*</span>}</span>{children}</label> }
