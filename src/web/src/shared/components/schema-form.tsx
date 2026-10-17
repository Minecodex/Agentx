import { Braces, RotateCcw, Upload } from 'lucide-react'
import { useEffect, useId, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { i18n } from '../../app/i18n'
import { Button } from '../ui/button'
import { Input } from '../ui/input'
import { Select } from '../ui/select'
import { Textarea } from '../ui/textarea'

export type JsonSchema = {
  type?: string
  title?: string
  description?: string
  default?: unknown
  enum?: unknown[]
  properties?: Record<string, JsonSchema>
  required?: string[]
  items?: JsonSchema
  minimum?: number
  maximum?: number
  minLength?: number
  maxLength?: number
  minItems?: number
  maxItems?: number
  multiline?: boolean
  format?: string
  'x-agentx-artifact'?: boolean
  'x-agentx-artifact-array'?: boolean
  'x-agentx-content-types'?: string[]
  'x-agentx-max-size-bytes'?: number
  'x-agentx-max-total-size-bytes'?: number
  'x-agentx-sensitive'?: boolean
}

export type ArtifactReference = {
  artifactId: string
  fileName?: string
  contentType?: string
  sizeBytes?: number
  sha256?: string
  type?: string
}

type Values = Record<string, unknown>

export function SchemaForm({ schema, values, onChange, onUpload, disabled = false, errors = {} }: {
  schema: JsonSchema
  values: Values
  onChange: (values: Values) => void
  onUpload?: (file: File) => Promise<ArtifactReference>
  disabled?: boolean
  errors?: Record<string, string>
}) {
  const fields = Object.entries(schema.properties ?? {})
  const required = new Set(schema.required ?? [])
  return <div className="grid gap-4 sm:grid-cols-2">{fields.map(([name, field]) => <SchemaField disabled={disabled} error={errors[name]} field={field} key={name} name={name} onChange={(value) => onChange({ ...values, [name]: value })} onUpload={onUpload} required={required.has(name)} value={values[name]} />)}</div>
}

export function SchemaFormWorkspace({ schema, value, onChange, onUpload, disabled = false }: {
  schema: JsonSchema
  value: Values
  onChange: (value: Values) => void
  onUpload?: (file: File) => Promise<ArtifactReference>
  disabled?: boolean
}) {
  const { t } = useTranslation()
  const [jsonMode, setJsonMode] = useState(false)
  const [jsonText, setJsonText] = useState(() => JSON.stringify(value, null, 2))
  const [jsonError, setJsonError] = useState<string>()
  useEffect(() => { if (!jsonMode) setJsonText(JSON.stringify(value, null, 2)) }, [value, jsonMode])
  const restore = () => { const defaults = schemaDefaults(schema); onChange(defaults); setJsonText(JSON.stringify(defaults, null, 2)); setJsonError(undefined) }
  const updateJson = (text: string) => {
    setJsonText(text)
    try {
      const parsed = JSON.parse(text) as unknown
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error(t('common.schemaForm.jsonObjectRequired'))
      onChange(parsed as Values)
      setJsonError(undefined)
    } catch (error) { setJsonError(error instanceof Error ? error.message : String(error)) }
  }
  return <div>
    <div className="mb-4 flex justify-end gap-2"><Button onClick={restore} size="sm" variant="ghost"><RotateCcw className="size-3.5" />{t('common.schemaForm.restoreDefaults')}</Button><Button aria-pressed={jsonMode} onClick={() => setJsonMode((current) => !current)} size="sm" variant="secondary"><Braces className="size-3.5" />{t('common.schemaForm.jsonMode')}</Button></div>
    {jsonMode ? <><Textarea aria-label={t('common.schemaForm.jsonInput')} className="min-h-[360px] font-mono text-[11px]" disabled={disabled} onChange={(event) => updateJson(event.target.value)} value={jsonText} />{jsonError && <p className="mt-2 text-xs text-danger">{jsonError}</p>}</> : <SchemaForm disabled={disabled} onChange={onChange} onUpload={onUpload} schema={schema} values={value} />}
  </div>
}

export function SchemaField({ name, field, value, required, onChange, onUpload, disabled, error }: {
  name: string
  field: JsonSchema
  value: unknown
  required: boolean
  onChange: (value: unknown) => void
  onUpload?: (file: File) => Promise<ArtifactReference>
  disabled: boolean
  error?: string
}) {
  const errorId = useId()
  const label = field.title || humanize(name)
  const wide = field.type === 'object' || field.type === 'array' || field.multiline || field['x-agentx-artifact']
  return <div className={wide ? 'sm:col-span-2' : undefined}><label className="mb-1.5 block text-xs font-medium"><span>{label}</span>{required && <span className="ml-1 text-danger">*</span>}{field.description && <span className="mt-0.5 block text-[10px] font-normal text-muted-foreground">{field.description}</span>}</label><SchemaControl disabled={disabled} error={error} errorId={errorId} field={field} label={label} name={name} onChange={onChange} onUpload={onUpload} required={required} value={value} />{error && <p className="mt-1 text-[10px] text-danger" id={errorId}>{error}</p>}</div>
}

function SchemaControl({ field, label, name, value, onChange, onUpload, disabled, required, error, errorId }: {
  field: JsonSchema
  label: string
  name: string
  value: unknown
  onChange: (value: unknown) => void
  onUpload?: (file: File) => Promise<ArtifactReference>
  disabled: boolean
  required: boolean
  error?: string
  errorId: string
}) {
  const { t } = useTranslation()
  const accessibility = { name, 'aria-invalid': Boolean(error), 'aria-describedby': error ? errorId : undefined, 'aria-required': required }
  if (field['x-agentx-artifact']) return <ArtifactInput disabled={disabled} field={field} label={label} onChange={onChange} onUpload={onUpload} value={value} />
  if (field.enum?.length) return <Select {...accessibility} aria-label={label} className="w-full" disabled={disabled} onValueChange={(selected) => onChange(field.enum?.find((item) => String(item) === selected))} options={field.enum.map((item) => ({ value: String(item), label: String(item) }))} value={value == null ? '' : String(value)} />
  if (field.type === 'boolean') return <label className="flex h-9 items-center gap-2 rounded-md border border-border bg-surface px-3 text-xs"><input {...accessibility} aria-label={label} checked={Boolean(value)} disabled={disabled} onChange={(event) => onChange(event.target.checked)} type="checkbox" />{value ? t('common.schemaForm.yes') : t('common.schemaForm.no')}</label>
  if (field.type === 'number' || field.type === 'integer') return <Input {...accessibility} aria-label={label} disabled={disabled} max={field.maximum} min={field.minimum} onChange={(event) => onChange(event.target.value ? Number(event.target.value) : undefined)} step={field.type === 'integer' ? 1 : 'any'} type="number" value={typeof value === 'number' ? value : ''} />
  if (field.type === 'object' || field.type === 'array') return <JsonValueInput disabled={disabled} fallback={field.type === 'array' ? [] : {}} label={label} onChange={onChange} value={value} />
  if (field.multiline) return <Textarea {...accessibility} aria-label={label} className="min-h-28" disabled={disabled} maxLength={field.maxLength} minLength={field.minLength} onChange={(event) => onChange(event.target.value)} value={String(value ?? '')} />
  return <Input {...accessibility} aria-label={label} disabled={disabled} maxLength={field.maxLength} minLength={field.minLength} onChange={(event) => onChange(event.target.value)} type={field.format === 'password' ? 'password' : field.format === 'email' ? 'email' : 'text'} value={String(value ?? '')} />
}

function ArtifactInput({ field, value, onChange, onUpload, label, disabled }: {
  field: JsonSchema
  value: unknown
  onChange: (value: unknown) => void
  onUpload?: (file: File) => Promise<ArtifactReference>
  label: string
  disabled: boolean
}) {
  const { t } = useTranslation()
  const [uploading, setUploading] = useState(false)
  const [error, setError] = useState<string>()
  const multiple = Boolean(field['x-agentx-artifact-array'])
  const references = useMemo(() => multiple ? (Array.isArray(value) ? value as ArtifactReference[] : []) : (value ? [value as ArtifactReference] : []), [multiple, value])
  const upload = async (files: FileList | null) => {
    if (!files?.length || !onUpload) return
    const selected = Array.from(files)
    const nextError = validateArtifactSelection(field, references, selected)
    if (nextError) { setError(nextError); return }
    setUploading(true); setError(undefined)
    try {
      const uploaded = await Promise.all(selected.map(onUpload))
      onChange(multiple ? [...references, ...uploaded] : uploaded[0])
    } catch (next) { setError(next instanceof Error ? next.message : String(next)) } finally { setUploading(false) }
  }
  return <div className="rounded-md border border-dashed border-border p-3"><label className="flex cursor-pointer items-center justify-center gap-2 rounded-md bg-muted/50 px-3 py-3 text-xs text-muted-foreground"><Upload className="size-4" />{uploading ? t('common.schemaForm.uploading') : multiple ? t('common.schemaForm.selectFiles') : t('common.schemaForm.selectFile')}<input accept={field['x-agentx-content-types']?.join(',')} aria-label={label} className="sr-only" disabled={disabled || uploading || !onUpload} multiple={multiple} onChange={(event) => void upload(event.target.files)} type="file" /></label>{references.length > 0 && <div className="mt-2 space-y-1">{references.map((reference, index) => <div className="flex items-center gap-2 rounded bg-muted/40 px-2 py-1.5 text-[10px]" key={reference.artifactId}><span className="min-w-0 flex-1 truncate">{reference.fileName ?? reference.artifactId}</span><Button aria-label={t('common.schemaForm.removeFile')} onClick={() => onChange(multiple ? references.filter((_, itemIndex) => itemIndex !== index) : undefined)} size="sm" variant="ghost">{t('common.schemaForm.remove')}</Button></div>)}</div>}{error && <p className="mt-2 text-[10px] text-danger">{error}</p>}</div>
}

function JsonValueInput({ value, fallback, onChange, label, disabled }: { value: unknown; fallback: unknown; onChange: (value: unknown) => void; label: string; disabled: boolean }) {
  const { t } = useTranslation()
  const [text, setText] = useState(() => JSON.stringify(value ?? fallback, null, 2))
  const [error, setError] = useState(false)
  useEffect(() => setText(JSON.stringify(value ?? fallback, null, 2)), [value, fallback])
  return <><Textarea aria-invalid={error} aria-label={label} className="min-h-28 font-mono text-[11px]" disabled={disabled} onChange={(event) => { setText(event.target.value); try { onChange(JSON.parse(event.target.value)); setError(false) } catch { setError(true) } }} value={text} />{error && <p className="mt-1 text-[10px] text-danger">{t('common.schemaForm.invalidJson')}</p>}</>
}

export function schemaDefaults(schema: JsonSchema): Values {
  return Object.fromEntries(Object.entries(schema.properties ?? {}).flatMap(([name, field]) => {
    const value = defaultValue(field)
    return value === undefined ? [] : [[name, value]]
  }))
}

function defaultValue(schema: JsonSchema): unknown {
  if (schema.default !== undefined) return structuredClone(schema.default)
  if (schema.type === 'object' && schema.properties) {
    const nested = schemaDefaults(schema)
    return Object.keys(nested).length ? nested : undefined
  }
  return undefined
}

export function validateSchemaValues(schema: JsonSchema, values: Values) {
  const errors: Record<string, string> = {}
  const required = new Set(schema.required ?? [])
  for (const [name, field] of Object.entries(schema.properties ?? {})) {
    const value = values[name]
    if (required.has(name) && (value === undefined || value === null || value === '')) errors[name] = i18n.t('common.schemaForm.required')
    else if (typeof value === 'number' && field.type === 'integer' && !Number.isInteger(value)) errors[name] = i18n.t('common.schemaForm.integerRequired')
    else if (typeof value === 'string' && field.minLength != null && value.length < field.minLength) errors[name] = i18n.t('common.schemaForm.minimumLength', { count: field.minLength })
    else if (field['x-agentx-artifact'] && field['x-agentx-artifact-array'] && (required.has(name) || (field.minItems ?? 0) > 0) && (!Array.isArray(value) || value.length < Math.max(1, field.minItems ?? 0))) errors[name] = i18n.t('common.schemaForm.fileRequired')
    else if (field['x-agentx-artifact'] && field['x-agentx-artifact-array'] && Array.isArray(value) && field.maxItems != null && value.length > field.maxItems) errors[name] = i18n.t('common.schemaForm.tooManyFiles', { count: field.maxItems })
  }
  return errors
}

function validateArtifactSelection(field: JsonSchema, current: ArtifactReference[], selected: File[]) {
  if (field.maxItems != null && current.length + selected.length > field.maxItems) return i18n.t('common.schemaForm.tooManyFiles', { count: field.maxItems })
  const disallowed = selected.find((file) => !contentTypeAllowed(file.type || 'application/octet-stream', field['x-agentx-content-types']))
  if (disallowed) return i18n.t('common.schemaForm.fileTypeNotAllowed', { name: disallowed.name })
  const oversized = selected.find((file) => field['x-agentx-max-size-bytes'] != null && file.size > field['x-agentx-max-size-bytes']!)
  if (oversized) return i18n.t('common.schemaForm.fileTooLarge', { name: oversized.name, size: field['x-agentx-max-size-bytes'] })
  const total = current.reduce((sum, file) => sum + (file.sizeBytes ?? 0), 0) + selected.reduce((sum, file) => sum + file.size, 0)
  if (field['x-agentx-max-total-size-bytes'] != null && total > field['x-agentx-max-total-size-bytes']) return i18n.t('common.schemaForm.filesTooLarge', { size: field['x-agentx-max-total-size-bytes'] })
  return undefined
}

function contentTypeAllowed(contentType: string, allowed?: string[]) {
  if (!allowed?.length) return true
  return allowed.some((value) => value === contentType || (value.endsWith('/*') && contentType.startsWith(value.slice(0, -1))))
}

function humanize(value: string) { return value.replace(/([a-z0-9])([A-Z])/g, '$1 $2').replace(/[_-]+/g, ' ').replace(/^./, (character) => character.toUpperCase()) }
