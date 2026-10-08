import { Plus, Trash2 } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { ApiClientError } from '../../shared/api/client'
import type { ApplicationDeployment, WorkflowVersion } from '../../shared/api/types'
import { Button } from '../../shared/ui/button'
import { Input } from '../../shared/ui/input'
import { Select } from '../../shared/ui/select'
import type { WebhookProviderTemplate } from '../../shared/api/types'
import { effectiveChannelMode } from './webhook-channel-fields'

export function channelSchemas(deployment: ApplicationDeployment | undefined, versions: WorkflowVersion[] | undefined) {
  if (deployment) return { input: deployment.inputSchema, output: deployment.outputSchema, version: deployment.workflowVersionNumber }
  if (!versions?.length) return undefined
  const version = versions.reduce((latest, item) => item.versionNumber > latest.versionNumber ? item : latest)
  const definition = version.definition as { start: { inputs: unknown }; end: { outputs: Record<string, { schema: unknown }> } }
  return { input: definition.start.inputs, output: { properties: Object.fromEntries(Object.entries(definition.end.outputs).map(([name, field]) => [name, field.schema])) }, version: version.versionNumber }
}

function requiredInputs(schema: unknown): string[] {
  const required = (schema as { required?: unknown } | undefined)?.required
  return Array.isArray(required) ? required.filter((name): name is string => typeof name === 'string' && schemaProperties(schema).includes(name)) : []
}

export function channelFieldError(field: string, message: string) {
  return new ApiClientError(422, { code: 'CHANNEL_CONFIGURATION_INVALID', message, requestId: '', fieldErrors: [{ field, code: 'CHANNEL_CONFIGURATION_INVALID', message }] })
}

export function channelInputs(values: Record<string, string>, schema: unknown, t: (key: string, options?: Record<string, unknown>) => string) {
  const mappings = parseMappings(values.inputMappings)
  const fixedInputs = parseFixedInputs(values.fixedInputs, schema)
  const targets = schemaProperties(schema)
  const occupied = new Set(Object.keys(fixedInputs))
  for (const row of mappings) {
    if (!targets.includes(row.target) || occupied.has(row.target)) throw channelFieldError('inputMappings', t('applications.mappingTargetInvalid'))
    occupied.add(row.target)
    if (!row.source.trim() || row.source === 'raw.') throw channelFieldError('inputMappings', t('applications.mappingSourceRequired', { field: row.target }))
  }
  const missing = requiredInputs(schema).filter((name) => !occupied.has(name))
  if (missing.length) throw channelFieldError('inputMappings', t('applications.requiredMappingsMissing', { fields: missing.join(', ') }))
  return { inputMappings: mappings, fixedInputs }
}

export function initialChannelMappings(schema: unknown, mappings: MappingValue[] = [], fixed: unknown = {}) {
  const occupied = new Set([...mappings.map((row) => row.target), ...Object.keys((fixed ?? {}) as object)])
  return [...mappings, ...defaultChannelMappings(schema).filter((row) => !occupied.has(row.target))]
}

/// Coerces fixed input strings to the Workflow Start Input schema type so
/// number/boolean parameters can be configured from a plain text field.
export function parseFixedInputs(value: string, schema: unknown) {
  try {
    const parsed = JSON.parse(value || '{}') as Record<string, unknown>
    const result: Record<string, unknown> = {}
    for (const [key, item] of Object.entries(parsed)) {
      if (!key.trim()) continue
      const type = schemaPropertyType(schema, key)
      if (typeof item === 'string' && (type === 'number' || type === 'integer')) {
        const numeric = Number(item)
        result[key] = item.trim() !== '' && Number.isFinite(numeric) ? numeric : item
      } else if (typeof item === 'string' && type === 'boolean' && (item === 'true' || item === 'false')) {
        result[key] = item === 'true'
      } else {
        result[key] = item
      }
    }
    return result
  } catch { return {} }
}

export function schemaProperties(schema: unknown): string[] {
  if (!schema || typeof schema !== 'object' || Array.isArray(schema)) return []
  const properties = (schema as { properties?: unknown }).properties
  if (!properties || typeof properties !== 'object' || Array.isArray(properties)) return []
  return Object.keys(properties)
}

function schemaPropertyType(schema: unknown, key: string): string | undefined {
  if (!schema || typeof schema !== 'object' || Array.isArray(schema)) return undefined
  const properties = (schema as { properties?: Record<string, { type?: unknown }> }).properties
  const type = properties?.[key]?.type
  return typeof type === 'string' ? type : undefined
}

export function defaultChannelMappings(schema: unknown) {
  return requiredInputs(schema).map((target) => ({ source: '', target, missingPolicy: 'error' }))
}

type MappingValue = { source: string; target: string; missingPolicy?: string }

function parseMappings(value: string): MappingValue[] {
  try {
    const parsed = JSON.parse(value || '[]') as unknown
    return Array.isArray(parsed) ? parsed.filter((item): item is MappingValue => Boolean(item) && typeof item === 'object' && typeof (item as MappingValue).source === 'string' && typeof (item as MappingValue).target === 'string') : []
  } catch { return [] }
}

/// Standardized Trigger Context sources shared by every provider. Provider
/// payload fields beyond these are addressed as raw.<dotted.path>.
function standardSourceOptions(t: (key: string) => string): Array<{ value: string; label: string }> {
  return [
    { value: 'message.text', label: t('applications.sourceMessageText') },
    { value: 'conversation.id', label: t('applications.sourceConversationId') },
    { value: 'conversation.name', label: t('applications.sourceConversationName') },
    { value: 'conversation.type', label: t('applications.sourceConversationType') },
    { value: 'sender.id', label: t('applications.sourceSenderId') },
    { value: 'sender.name', label: t('applications.sourceSenderName') },
    { value: 'provider', label: t('applications.sourceProvider') },
    { value: 'provider_event_id', label: t('applications.sourceProviderEventId') },
  ]
}

/// Mapping rows read workflow-input-first: pick the Workflow Start Input to
/// fill, then pick where its value comes from — a standardized channel field
/// or a provider payload field from the template catalog. Anything else
/// (e.g. an agentx caller's own JSON) can be typed as a raw.* path.
export function MappingEditor({ schema, templates, provider, mode, value, fixedInputs = '{}', onChange }: { schema: unknown; templates: WebhookProviderTemplate[]; provider?: string; mode?: string; value: string; fixedInputs?: string; onChange: (value: string) => void }) {
  const { t } = useTranslation()
  const rows = parseMappings(value)
  const targets = schemaProperties(schema)
  const required = new Set(requiredInputs(schema))
  const fixed = Object.keys(parseFixedInputs(fixedInputs, schema))
  const available = targets.filter((target) => !fixed.includes(target) && !rows.some((row) => row.target === target))
  const effectiveMode = effectiveChannelMode({ providerType: provider ?? '', channelMode: mode ?? 'callback' })
  const providerSources = templates.find((item) => item.provider === provider && item.mode === effectiveMode)?.mappingSources ?? []
  const options = [
    ...standardSourceOptions(t),
    ...providerSources.map((source) => ({ value: source, label: source })),
  ]
  const update = (index: number, patch: Partial<MappingValue>) => onChange(JSON.stringify(rows.map((row, current) => current === index ? { ...row, ...patch } : row)))
  return <div className="space-y-2">
    <input name="inputMappings" type="hidden" value={value} />
    {rows.map((row, index) => {
      const knownSource = !row.source || options.some((option) => option.value === row.source)
      return <div className="grid grid-cols-[1fr_1fr_auto] gap-2" key={index}>
        <Select aria-label={t('applications.workflowInput')} aria-required={required.has(row.target)} className="min-w-0" onValueChange={(target) => update(index, { target })} options={targets.map((target) => ({ value: target, label: required.has(target) ? `${target} *` : target, disabled: fixed.includes(target) || rows.some((other, current) => current !== index && other.target === target) }))} placeholder={row.target || t('applications.selectWorkflowInput')} value={targets.includes(row.target) ? row.target : ''} />
        {knownSource
          ? <Select aria-label={t('applications.sourceField')} aria-required={required.has(row.target)} className="min-w-0" onValueChange={(source) => update(index, { source })} options={options} placeholder={t('applications.selectSourceField')} value={row.source} />
          : <Input aria-label={t('applications.sourceField')} className="min-w-0" onChange={(event) => update(index, { source: event.target.value })} placeholder="raw.senderNick" value={row.source} />}
        <Button aria-label={t('applications.removeMapping')} onClick={() => onChange(JSON.stringify(rows.filter((_, current) => current !== index)))} size="icon" type="button" variant="ghost"><Trash2 className="size-3.5" /></Button>
      </div>
    })}
    <div className="flex gap-2">
      <Button aria-label={t('applications.addMapping')} disabled={!available.length} onClick={() => onChange(JSON.stringify([...rows, { source: '', target: available[0], missingPolicy: 'error' }]))} size="sm" type="button" variant="secondary"><Plus className="size-3.5" />{t('applications.addMapping')}</Button>
      <Button aria-label={t('applications.addCustomMapping')} disabled={!available.length} onClick={() => onChange(JSON.stringify([...rows, { source: 'raw.', target: available[0], missingPolicy: 'error' }]))} size="sm" type="button" variant="secondary"><Plus className="size-3.5" />{t('applications.addCustomMapping')}</Button>
    </div>
    <p className="text-[11px] leading-5 text-muted-foreground">{t('applications.mappingHint')}</p>
  </div>
}

/// Fixed inputs are workflow-input-first as well: pick the Workflow Start
/// Input, then type the constant value (coerced to the schema type on save).
export function FixedInputsEditor({ schema, value, mappings = '[]', onChange }: { schema: unknown; value: string; mappings?: string; onChange: (value: string) => void }) {
  const { t } = useTranslation()
  const targets = schemaProperties(schema)
  const mapped = parseMappings(mappings).map((row) => row.target)
  let entries: Array<[string, string]> = []
  try { const parsed = JSON.parse(value || '{}') as Record<string, unknown>; entries = Object.entries(parsed).map(([key, item]) => [key, typeof item === 'string' ? item : JSON.stringify(item)]) } catch { entries = [] }
  const update = (next: Array<[string, string]>) => { const object: Record<string, string> = {}; for (const [key, item] of next) object[key] = item; onChange(JSON.stringify(object)) }
  const available = targets.filter((target) => !mapped.includes(target) && !entries.some(([key]) => key === target))
  return <div className="space-y-2">
    <input name="fixedInputs" type="hidden" value={value} />
    {entries.map(([key, item], index) => <div className="grid grid-cols-[1fr_1fr_auto] gap-2" key={index}>
      <Select aria-label={t('applications.workflowInput')} className="min-w-0" onValueChange={(nextKey) => { const next = entries.slice(); next[index] = [nextKey, item]; update(next) }} options={targets.map((target) => ({ value: target, label: target, disabled: mapped.includes(target) || entries.some(([other], current) => current !== index && other === target) }))} value={targets.includes(key) ? key : ''} placeholder={key || t('applications.selectWorkflowInput')} />
      <Input aria-label={t('applications.fixedInputValue')} onChange={(event) => { const next = entries.slice(); next[index] = [key, event.target.value]; update(next) }} placeholder={typePlaceholder(schema, key)} value={item} />
      <Button aria-label={t('applications.removeFixedInput')} onClick={() => update(entries.filter((_, current) => current !== index))} size="icon" type="button" variant="ghost"><Trash2 className="size-3.5" /></Button>
    </div>)}
    <Button aria-label={t('applications.addFixedInput')} disabled={!available.length} onClick={() => update([...entries, [available[0], '']])} size="sm" type="button" variant="secondary"><Plus className="size-3.5" />{t('applications.addFixedInput')}</Button>
  </div>
}

function typePlaceholder(schema: unknown, key: string) {
  const type = schemaPropertyType(schema, key)
  return type === 'number' || type === 'integer' ? '42' : type === 'boolean' ? 'true' : 'customer_service'
}
