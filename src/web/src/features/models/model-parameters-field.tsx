import { useTranslation } from 'react-i18next'

import { Select } from '../../shared/ui/select'
import { Textarea } from '../../shared/ui/textarea'
import { effortLabels } from './model-reasoning'

export function ModelParametersField({ value, onChange, error }: { value: string; onChange: (value: string) => void; error?: string }) {
  const { t } = useTranslation()
  let parameters: Record<string, unknown> | undefined
  try {
    const parsed: unknown = JSON.parse(value)
    if (parsed !== null && typeof parsed === 'object' && !Array.isArray(parsed)) parameters = parsed as Record<string, unknown>
  } catch { /* Keep incomplete JSON editable without replacing the user's text. */ }
  const configured = parameters?.reasoning_effort
  const effort = typeof configured === 'string' ? `effort:${configured}` : parameters && Object.hasOwn(parameters, 'reasoning_effort') ? 'advanced' : 'default'
  const options = [
    { value: 'default', label: t('models.reasoningDefault') },
    ...Object.entries(effortLabels).map(([item, label]) => ({ value: `effort:${item}`, label: t(label) })),
  ]
  if (!options.some((option) => option.value === effort)) options.push({ value: effort, label: typeof configured === 'string' && configured ? configured : t('models.reasoningCustom') })
  const selectEffort = (selected: string) => {
    if (!parameters || selected === 'advanced') return
    const next = { ...parameters }
    if (selected === 'default') delete next.reasoning_effort
    else next.reasoning_effort = selected.slice('effort:'.length)
    onChange(JSON.stringify(next, null, 2))
  }
  return <div className="space-y-4">
    <label className="block text-xs">
      <span className="mb-2 block font-medium">{t('models.reasoningEffort')}</span>
      <Select aria-label={t('models.reasoningEffort')} className="w-full" disabled={!parameters} onValueChange={selectEffort} options={options} value={effort} />
      <span className="mt-1.5 block text-[11px] leading-5 text-muted-foreground">{t('models.reasoningHelp')}</span>
    </label>
    <label className="block text-xs">
      <span className="mb-2 block font-medium">{t('models.defaultParameters')}</span>
      <Textarea aria-invalid={Boolean(error)} aria-label={t('models.defaultParameters')} onChange={(event) => onChange(event.target.value)} value={value} />
    </label>
  </div>
}
