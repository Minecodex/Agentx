import { i18n } from '../../app/i18n'
import { ApiClientError } from '../../shared/api/client'
import { schemaDefaults, validateSchemaValues, type JsonSchema } from '../../shared/components/schema-form'

export function initialScheduleValue(field: JsonSchema): unknown {
  if (field.default !== undefined) return structuredClone(field.default)
  if (field.type === 'object') return initialScheduleInputs(field)
  if (field.type === 'array') return []
  return field.type === 'number' || field.type === 'integer' || field.type === 'boolean' ? null : ''
}

export function initialScheduleInputs(schema: JsonSchema): Record<string, unknown> {
  return {
    ...Object.fromEntries((schema.required ?? []).filter(name => schema.properties?.[name]).map(name => [name, initialScheduleValue(schema.properties![name])])),
    ...schemaDefaults(schema),
  }
}

export function scheduleInputErrors(schema: JsonSchema, values: Record<string, unknown>, prefix = ''): Record<string, string> {
  const errors = Object.fromEntries(Object.entries(validateSchemaValues(schema, values)).map(([name, error]) => [prefix + name, error]))
  for (const [name, value] of Object.entries(values)) {
    const path = prefix + name
    const field = schema.properties?.[name]
    const invalid = () => { errors[path] = i18n.t('applications.scheduleInputInvalid') }
    if (!field) { errors[path] = i18n.t('applications.scheduleInputUnknown'); continue }
    if (errors[path]) continue
    if (value == null) { invalid(); continue }
    if (field.enum && !field.enum.some(option => JSON.stringify(option) === JSON.stringify(value))) invalid()
    if (field.type === 'string') {
      if (typeof value !== 'string' || (field.maxLength != null && value.length > field.maxLength)) invalid()
    } else if (field.type === 'number' || field.type === 'integer') {
      if (typeof value !== 'number' || !Number.isFinite(value) || (field.minimum != null && value < field.minimum) || (field.maximum != null && value > field.maximum)) invalid()
    } else if (field.type === 'boolean') {
      if (typeof value !== 'boolean') invalid()
    } else if (field.type === 'object') {
      if (typeof value !== 'object' || Array.isArray(value)) invalid()
      else if (!field['x-agentx-artifact']) Object.assign(errors, scheduleInputErrors(field, value as Record<string, unknown>, `${path}.`))
    } else if (field.type === 'array') {
      if (!Array.isArray(value) || (field.minItems != null && value.length < field.minItems) || (field.maxItems != null && value.length > field.maxItems)) invalid()
      else if (field.items && !field['x-agentx-artifact']) value.forEach((item, index) => Object.assign(errors, scheduleInputErrors({ properties: { [index]: field.items! }, required: [String(index)] }, { [index]: item }, `${path}.`)))
    }
  }
  return errors
}

export function scheduleInput(value: string, schema: JsonSchema | undefined) {
  const input = JSON.parse(value) as Record<string, unknown>
  const errors = schema ? scheduleInputErrors(schema, input) : { input: i18n.t('applications.scheduleSchemaMissing') }
  if (Object.keys(errors).length) {
    const message = Object.entries(errors).map(([name, error]) => `${name}: ${error}`).join('；')
    throw new ApiClientError(422, { code: 'SCHEDULE_INPUT_INVALID', message, requestId: '', fieldErrors: [{ field: 'input', code: 'SCHEDULE_INPUT_INVALID', message }] })
  }
  return input
}
