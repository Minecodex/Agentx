import { Plus, Trash2 } from 'lucide-react'
import { useEffect, useRef } from 'react'
import { useTranslation } from 'react-i18next'

import { gatewayRequest } from '../../shared/api/client'
import { SchemaField, type ArtifactReference, type JsonSchema } from '../../shared/components/schema-form'
import { Button } from '../../shared/ui/button'
import { Select } from '../../shared/ui/select'
import { initialScheduleValue, scheduleInputErrors } from './schedule-input-values'

export function ScheduleInputsEditor({ schema, value, onChange, error }: { schema: JsonSchema; value: string; onChange: (value: string) => void; error?: string }) {
  const { t } = useTranslation()
  const root = useRef<HTMLDivElement>(null)
  const input = JSON.parse(value) as Record<string, unknown>
  const entries = Object.entries(input)
  const properties = schema.properties ?? {}
  const required = new Set(schema.required ?? [])
  const available = Object.keys(properties).filter(name => !Object.hasOwn(input, name))
  const errors = error ? scheduleInputErrors(schema, input) : {}
  useEffect(() => { if (error) (root.current?.querySelector<HTMLElement>('[aria-invalid="true"]') ?? root.current)?.focus() }, [error])
  const update = (next: Record<string, unknown>) => onChange(JSON.stringify(next))
  const remove = (name: string) => update(Object.fromEntries(entries.filter(([key]) => key !== name)))
  return <div className="space-y-3" ref={root} tabIndex={-1}>
    {entries.map(([name, item]) => {
      const field = properties[name]
      const wide = field && !field['x-agentx-artifact'] && !field.enum?.length && (field.type === 'object' || field.type === 'array')
      const control = field ? <ScheduleValueField errors={errors} field={field} name={name} onChange={next => update({ ...input, [name]: next ?? null })} required={required.has(name)} value={item} /> : <p className="text-xs text-danger">{t('applications.scheduleInputUnknown')}</p>
      return <div aria-label={t('applications.scheduleInputRow', { name })} className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] items-start gap-3 rounded-lg border border-border p-3" key={name} role="group">
        <div className={wide ? 'col-span-2' : undefined}><p className="mb-1.5 text-xs font-medium">{t('applications.workflowInput')}</p><Select aria-label={t('applications.workflowInput')} className="w-full min-w-0" onValueChange={next => update(Object.fromEntries(entries.map(([key, current]) => key === name ? [next, initialScheduleValue(properties[next])] : [key, current])))} options={Object.entries(properties).map(([key, property]) => ({ value: key, label: `${property.title ? `${property.title} (${key})` : key}${required.has(key) ? ' *' : ''}`, disabled: key !== name && Object.hasOwn(input, key) }))} placeholder={name} value={field ? name : ''} /></div>
        {!wide && <div className="min-w-0">{control}</div>}
        <Button aria-label={t('applications.removeScheduleInput', { name })} className="mt-5" onClick={() => remove(name)} size="icon" type="button" variant="ghost"><Trash2 className="size-3.5" /></Button>
        {wide && <div className="col-span-3">{control}</div>}
      </div>
    })}
    {available.length > 0 && <Select aria-label={t('applications.addScheduleInput')} className="w-full" onValueChange={name => update({ ...input, [name]: initialScheduleValue(properties[name]) })} options={available.map(name => ({ value: name, label: `${properties[name].title ? `${properties[name].title} (${name})` : name}${required.has(name) ? ' *' : ''}` }))} placeholder={t('applications.addScheduleInput')} value="" />}
    {!Object.keys(properties).length && <p className="text-xs text-muted-foreground">{t('applications.noScheduleInputs')}</p>}
  </div>
}

function ScheduleValueField({ field, name, value, required, onChange, errors }: { field: JsonSchema; name: string; value: unknown; required: boolean; onChange: (value: unknown) => void; errors: Record<string, string> }) {
  const { t } = useTranslation()
  const label = field.title || name
  if (field['x-agentx-artifact'] || field.enum?.length || (field.type !== 'object' && field.type !== 'array' && field.type !== 'boolean')) {
    return <SchemaField disabled={false} error={errors[name]} field={{ ...field, title: label }} name={`input.${name}`} onChange={onChange} onUpload={uploadArtifact} required={required} value={value} />
  }
  if (field.type === 'boolean') return <div><label className="mb-1.5 block text-xs font-medium">{label}{required && <span className="ml-1 text-danger">*</span>}</label><Select aria-describedby={errors[name] ? `schedule-${name}-error` : undefined} aria-invalid={Boolean(errors[name])} aria-label={label} aria-required={required} className="w-full" name={`input.${name}`} onValueChange={next => onChange(next === 'true')} options={[{ value: 'true', label: t('common.schemaForm.yes') }, { value: 'false', label: t('common.schemaForm.no') }]} value={typeof value === 'boolean' ? String(value) : ''} />{field.description && <p className="mt-1 text-xs text-muted-foreground">{field.description}</p>}{errors[name] && <p className="mt-1 text-xs text-danger" id={`schedule-${name}-error`}>{errors[name]}</p>}</div>
  const object = (value ?? {}) as Record<string, unknown>
  const items = Array.isArray(value) ? value : []
  return <fieldset className="min-w-0 space-y-3 rounded-md border border-border p-3"><legend className="px-1 text-xs font-medium">{label}{required && <span className="ml-1 text-danger">*</span>}</legend>
    {field.description && <p className="text-xs text-muted-foreground">{field.description}</p>}
    {field.type === 'object' ? Object.entries(field.properties ?? {}).map(([key, child]) => <ScheduleValueField errors={errors} field={child} key={key} name={`${name}.${key}`} onChange={next => onChange({ ...object, [key]: next ?? null })} required={Boolean(field.required?.includes(key))} value={object[key]} />) : <>
      {items.map((item, index) => <div className="flex items-start gap-2" key={index}><div className="min-w-0 flex-1"><ScheduleValueField errors={errors} field={field.items ?? { type: 'string' }} name={`${name}.${index}`} onChange={next => onChange(items.map((current, itemIndex) => itemIndex === index ? next ?? null : current))} required value={item} /></div><Button aria-label={t('applications.removeScheduleInput', { name: `${name}.${index}` })} onClick={() => onChange(items.filter((_, itemIndex) => itemIndex !== index))} size="icon" type="button" variant="ghost"><Trash2 className="size-3.5" /></Button></div>)}
      <Button onClick={() => onChange([...items, initialScheduleValue(field.items ?? { type: 'string' })])} size="sm" type="button" variant="secondary"><Plus className="size-3.5" />{t('applications.addScheduleItem')}</Button>
    </>}
    {errors[name] && <p className="text-xs text-danger">{errors[name]}</p>}
  </fieldset>
}

async function uploadArtifact(file: File) {
  const body = new FormData()
  body.append('file', file)
  const uploaded = await gatewayRequest<ArtifactReference>('/artifacts', { method: 'POST', headers: { 'Idempotency-Key': crypto.randomUUID() }, body })
  return { ...uploaded, fileName: file.name }
}
