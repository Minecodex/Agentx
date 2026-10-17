import type { ApplicationSchedule } from '../../shared/api/types'
import type { EntityFormField } from '../../shared/components/entity-form-dialog'
import type { JsonSchema } from '../../shared/components/schema-form'
import { initialScheduleInputs } from './schedule-input-values'
import { ScheduleInputsEditor } from './schedule-inputs-editor'

export function scheduleFields(t: (key: string) => string, schema: JsonSchema | undefined, value?: ApplicationSchedule | null): EntityFormField[] {
  return [
    { name: 'name', label: t('common.name'), required: true, defaultValue: value?.name ?? '' },
    { name: 'cron', label: 'Cron', defaultValue: value?.cronExpression ?? '0 9 * * 1', required: true },
    { name: 'timezone', label: t('applications.timezone'), defaultValue: value?.timezone ?? 'Asia/Shanghai', required: true },
    { name: 'misfirePolicy', label: t('applications.misfirePolicy'), type: 'select', defaultValue: value?.misfirePolicy ?? 'fire_once', required: true, options: ['fire_once', 'skip'].map(item => ({ value: item, label: t(`applications.${item}`) })) },
    { name: 'input', label: t('applications.scheduleInputs'), defaultValue: JSON.stringify(value?.input ?? (schema ? initialScheduleInputs(schema) : {})), render: ({ value: input, update, error }) => schema ? <ScheduleInputsEditor error={error} onChange={update} schema={schema} value={input} /> : <p className="text-xs text-danger">{t('applications.scheduleSchemaMissing')}</p> },
    ...(value ? [{ name: 'status', label: t('common.status'), type: 'select' as const, defaultValue: value.status, required: true, options: ['active', 'disabled'].map(item => ({ value: item, label: t(`applications.${item}`) })) }] : []),
  ]
}
