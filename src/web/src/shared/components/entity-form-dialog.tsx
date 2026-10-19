import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import { ApiClientError, apiFieldErrors } from '../api/client'
import { Button } from '../ui/button'
import { Dialog, DialogContent } from '../ui/dialog'
import { Input } from '../ui/input'
import { Select } from '../ui/select'
import { Textarea } from '../ui/textarea'

export type EntityFormField = {
  name: string
  label: string
  type?: 'text' | 'password' | 'textarea' | 'select' | 'number' | 'checkbox'
  options?: Array<{ value: string; label: string }>
  defaultValue?: string
  max?: number
  maxLength?: number
  min?: number
  placeholder?: string
  required?: boolean
  step?: number | 'any'
  description?: string
  apiName?: string
  render?: (props: {
    value: string
    values: Record<string, string>
    update: (value: string) => void
    error?: string
  }) => ReactNode
  visible?: (values: Record<string, string>) => boolean
}

type Props = {
  cancelLabel: string
  fields: EntityFormField[]
  onClose: () => void
  onSubmit: (values: Record<string, string>) => Promise<void>
  open: boolean
  submitLabel: string
  title: string
  wide?: boolean
}

export function EntityFormDialog({ cancelLabel, fields, onClose, onSubmit, open, submitLabel, title, wide: wideLayout }: Props) {
  const { t } = useTranslation()
  const [values, setValues] = useState<Record<string, string>>(() => Object.fromEntries(fields.map((field) => [field.name, field.defaultValue ?? ''])))
  const [error, setError] = useState('')
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({})
  const [pending, setPending] = useState(false)
  const formRef = useRef<HTMLFormElement>(null)
  const fieldsRef = useRef(fields)
  fieldsRef.current = fields
  useEffect(() => {
    if (!open) return
    setValues(Object.fromEntries(fieldsRef.current.map((field) => [field.name, field.defaultValue ?? ''])))
    setError('')
    setFieldErrors({})
  }, [open])
  const update = (name: string, value: string) => {
    setValues((current) => ({ ...current, [name]: value }))
    setFieldErrors((current) => { const next = { ...current }; delete next[name]; return next })
  }
  const submit = async (event: FormEvent) => {
    event.preventDefault()
    const invalidCustom = [...(formRef.current?.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>('input,textarea') ?? [])].find(control => control.validity.customError)
    if (invalidCustom) { invalidCustom.reportValidity(); invalidCustom.focus(); return }
    const requiredErrors = Object.fromEntries(fieldsRef.current
      .filter((field) => (field.visible?.(values) ?? true) && field.required && (field.type === 'checkbox' ? values[field.name] !== 'true' : !values[field.name]?.trim()))
      .map((field) => [field.name, t('errors.requiredField')]))
    if (Object.keys(requiredErrors).length) {
      setError('')
      setFieldErrors(requiredErrors)
      focusFirstError(requiredErrors)
      return
    }
    setPending(true)
    setError('')
    setFieldErrors({})
    try { await onSubmit(values); onClose() } catch (value) {
      const apiErrors = apiFieldErrors(value)
      const mappedErrors: Record<string, string> = {}
      const unmatched = new Set(Object.keys(apiErrors))
      for (const field of fieldsRef.current) {
        const apiName = field.apiName ?? field.name
        const match = Object.keys(apiErrors).find((name) => name === apiName || name.split('.').at(-1) === apiName)
        if (match) { mappedErrors[field.name] = apiErrors[match]; unmatched.delete(match) }
      }
      setFieldErrors(mappedErrors)
      if (!Object.keys(mappedErrors).length || unmatched.size || !(value instanceof ApiClientError)) setError(value instanceof Error ? value.message : String(value))
      focusFirstError(mappedErrors)
    } finally { setPending(false) }
  }
  const focusFirstError = (errors: Record<string, string>) => requestAnimationFrame(() => formRef.current?.querySelector<HTMLElement>(`[name="${CSS.escape(Object.keys(errors)[0] ?? '')}"]`)?.focus())
  const wide = wideLayout ?? fields.length > 6
  return <Dialog onOpenChange={(value) => { if (!value) onClose() }} open={open}>
    <DialogContent className={wide ? 'w-[min(920px,calc(100vw-48px))] p-6 [&>div]:max-h-[calc(100dvh-82px)]' : 'p-6 [&>div]:max-h-[calc(100dvh-82px)]'} title={title}>
      <h2 className="text-lg font-semibold">{title}</h2>
      <form className={wide ? 'mt-5 grid grid-cols-2 gap-x-5 gap-y-4' : 'mt-5 space-y-4'} noValidate onSubmit={(event) => void submit(event)} ref={formRef}>
        {fields.map((field) => (field.visible?.(values) ?? true) ? <FieldContainer asLabel={!field.render} className={(field.type === 'textarea' || field.type === 'checkbox' || field.render) && wide ? 'col-span-2 block text-xs' : 'block text-xs'} key={field.name}>
          {field.type !== 'checkbox' && <span className="mb-2 block font-medium">{field.label}{field.required && <span aria-hidden="true" className="ml-1 text-danger">*</span>}</span>}
          {field.render ? field.render({ value: values[field.name] ?? '', values, update: (value) => update(field.name, value), error: fieldErrors[field.name] })
            : field.type === 'checkbox' ? <span className="flex items-start gap-3 rounded-lg border border-border/70 bg-muted/30 p-3"><input aria-describedby={fieldErrors[field.name] ? `${field.name}-error` : undefined} aria-invalid={Boolean(fieldErrors[field.name])} aria-label={field.label} aria-required={field.required} checked={values[field.name] === 'true'} className="mt-0.5 size-4 shrink-0 accent-primary" name={field.name} onChange={(event) => update(field.name, String(event.target.checked))} type="checkbox" /><span><span className="block font-medium">{field.label}{field.required && <span aria-hidden="true" className="ml-1 text-danger">*</span>}</span>{field.description && <span className="mt-1 block text-[11px] leading-5 text-muted-foreground">{field.description}</span>}</span></span>
            : field.type === 'select' ? <Select aria-describedby={fieldErrors[field.name] ? `${field.name}-error` : undefined} aria-invalid={Boolean(fieldErrors[field.name])} aria-label={field.label} aria-required={field.required} className="w-full" name={field.name} onValueChange={(value) => update(field.name, value)} options={field.options ?? []} value={values[field.name]} />
            : field.type === 'textarea' ? <Textarea aria-describedby={fieldErrors[field.name] ? `${field.name}-error` : undefined} aria-invalid={Boolean(fieldErrors[field.name])} aria-label={field.label} maxLength={field.maxLength} name={field.name} onChange={(event) => update(field.name, event.target.value)} placeholder={field.placeholder} required={field.required} value={values[field.name]} />
              : <Input aria-describedby={fieldErrors[field.name] ? `${field.name}-error` : undefined} aria-invalid={Boolean(fieldErrors[field.name])} aria-label={field.label} max={field.max} maxLength={field.maxLength} min={field.type === 'number' ? (field.min ?? 0) : undefined} name={field.name} onChange={(event) => update(field.name, event.target.value)} placeholder={field.placeholder} required={field.required} step={field.step} type={field.type ?? 'text'} value={values[field.name]} />}
          {field.description && field.type !== 'checkbox' && <span className="mt-1.5 block text-[11px] text-muted-foreground">{field.description}</span>}
          {fieldErrors[field.name] && <span className="mt-1.5 block text-xs text-danger" id={`${field.name}-error`}>{fieldErrors[field.name]}</span>}
        </FieldContainer> : null)}
        {error && <p className={wide ? 'col-span-2 rounded-lg border border-danger/20 bg-danger/10 p-3 text-xs text-danger' : 'rounded-lg border border-danger/20 bg-danger/10 p-3 text-xs text-danger'}>{error}</p>}
        <div className={wide ? 'col-span-2 flex justify-end gap-2' : 'flex justify-end gap-2'}><Button onClick={onClose} type="button" variant="ghost">{cancelLabel}</Button><Button disabled={pending} type="submit">{submitLabel}</Button></div>
      </form>
    </DialogContent>
  </Dialog>
}

function FieldContainer({ asLabel, children, className }: { asLabel: boolean; children: ReactNode; className: string }) {
  return asLabel ? <label className={className}>{children}</label> : <div className={className}>{children}</div>
}
