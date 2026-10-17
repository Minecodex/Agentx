import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, ShieldCheck, Trash2 } from 'lucide-react'
import { useMemo, useState, type FormEvent, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../api/client'
import type { Department, PageResponse, ResourceGrant, Workflow } from '../api/types'
import { Badge } from '../ui/badge'
import { Button } from '../ui/button'
import { Dialog, DialogContent } from '../ui/dialog'
import { Select } from '../ui/select'
import { useToast } from '../ui/toast'

const operationLabelKeys = {
  use: 'resourceGrants.use',
  view: 'resourceGrants.view',
  read: 'resourceGrants.read',
  write: 'resourceGrants.write',
  manage: 'resourceGrants.manage',
} as const

type GrantOperation = keyof typeof operationLabelKeys

type Resource = { id: string; name: string; resourceType: string }

export function ResourceGrantDialog({ onClose, open, resource }: { onClose: () => void; open: boolean; resource?: Resource }) {
  const { t } = useTranslation()
  const auth = useAuth()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const [subjectType, setSubjectType] = useState('workflow_service_identity')
  const [subjectId, setSubjectId] = useState('')
  const [operation, setOperation] = useState('use')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const [revokeGrant, setRevokeGrant] = useState<ResourceGrant>()
  const resourceType = resource?.resourceType ?? ''
  const resourceId = resource?.id ?? ''
  const grants = useQuery({ enabled: Boolean(resource), queryKey: ['resource-grants', resourceType, resourceId], queryFn: () => apiRequest<ResourceGrant[]>(`/resources/${resourceType}/${resourceId}/grants`) })
  const workflows = useQuery({ queryKey: ['workflows', 'grant-options'], queryFn: () => apiRequest<PageResponse<Workflow>>('/workflows?pageSize=100&status=active') })
  const departments = useQuery({ queryKey: ['departments', 'grant-options'], queryFn: () => apiRequest<Department[]>('/departments') })
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ['resource-grants', resourceType, resourceId] })
  const remove = useMutation({
    mutationFn: (grant: ResourceGrant) => apiRequest(`/resources/${resourceType}/${resourceId}/grants/${grant.id}`, { method: 'DELETE' }),
    onSuccess: async () => { setRevokeGrant(undefined); await Promise.all([invalidate(), queryClient.invalidateQueries({ queryKey: ['grantable-resources'] })]); showToast(t('resourceGrants.revoked')) },
    onError: (value: Error) => showToast(value.message),
  })
  const subjectOptions = useMemo(() => subjectType === 'department'
    ? (departments.data ?? []).map((item) => ({ value: item.id, label: item.name }))
    : (workflows.data?.items ?? []).map((item) => ({ value: item.serviceIdentityId, label: item.name })), [departments.data, subjectType, workflows.data?.items])
  const subjectNames = useMemo(() => new Map([
    ...(departments.data ?? []).map((item) => [item.id, item.name] as const),
    ...(workflows.data?.items ?? []).map((item) => [item.serviceIdentityId, item.name] as const),
  ]), [departments.data, workflows.data?.items])
  const operationOptions = operationsFor(resourceType).map((value) => ({ value, label: t(operationLabelKeys[value]) }))
  const effectiveOperation = operationOptions.some((item) => item.value === operation) ? operation : (operationOptions[0]?.value ?? 'use')
  const changeSubjectType = (value: string) => { setSubjectType(value); setSubjectId('') }
  const submit = async (event: FormEvent) => {
    event.preventDefault()
    if (!resource || !subjectId) { setError(t('resourceGrants.subjectRequired')); return }
    setPending(true); setError('')
    try {
      await apiRequest(`/resources/${resourceType}/${resourceId}/grants`, { method: 'POST', headers: { 'Idempotency-Key': crypto.randomUUID() }, body: jsonBody({ subjectType, subjectId, operation: effectiveOperation, resourceVersionId: null }) })
      await Promise.all([invalidate(), queryClient.invalidateQueries({ queryKey: ['grantable-resources'] })])
      setSubjectId(''); showToast(t('resourceGrants.grantAdded'))
    } catch (value) { setError(value instanceof Error ? value.message : String(value)) } finally { setPending(false) }
  }
  return <><Dialog onOpenChange={(value) => { if (!value) onClose() }} open={open}>
    <DialogContent className="w-[min(880px,calc(100vw-48px))] p-6" title={t('resourceGrants.manageTitle', { name: resource?.name ?? '' })}>
      <div className="flex items-start gap-3"><div className="grid size-9 place-items-center rounded-lg bg-primary/10 text-primary"><ShieldCheck className="size-4" /></div><div><h2 className="text-lg font-semibold">{t('resourceGrants.manageTitle', { name: resource?.name ?? '' })}</h2><p className="mt-1 text-xs text-muted-foreground">{t('resourceGrants.manageDescription')}</p></div></div>
      {auth.hasPermission('resource:grant') && <form className="mt-6 grid grid-cols-[180px_minmax(220px,1fr)_160px_auto] items-end gap-3 rounded-lg border border-border bg-canvas/40 p-4" onSubmit={(event) => void submit(event)}>
        <Field label={t('resourceGrants.subjectType')}><Select aria-label={t('resourceGrants.subjectType')} aria-required onValueChange={changeSubjectType} options={[{ value: 'workflow_service_identity', label: t('resourceGrants.workflow') }, { value: 'department', label: t('resourceGrants.department') }]} value={subjectType} /></Field>
        <Field label={t('resourceGrants.subject')}><Select aria-label={t('resourceGrants.subject')} aria-required onValueChange={setSubjectId} options={subjectOptions} placeholder={t('resourceGrants.select')} value={subjectId} /></Field>
        <Field label={t('resourceGrants.operation')}><Select aria-label={t('resourceGrants.operation')} aria-required onValueChange={setOperation} options={operationOptions} value={effectiveOperation} /></Field>
        <Button disabled={pending || !subjectId} type="submit"><Plus className="size-4" />{t('resourceGrants.add')}</Button>
        {error && <p className="col-span-full text-xs text-danger">{error}</p>}
      </form>}
      <div className="mt-5 overflow-hidden rounded-lg border border-border">
        <div className="grid grid-cols-[180px_minmax(0,1fr)_120px_48px] bg-muted/45 px-4 py-2 text-[10px] font-medium uppercase text-muted-foreground"><span>{t('resourceGrants.subjectType')}</span><span>{t('resourceGrants.subject')}</span><span>{t('resourceGrants.operation')}</span><span /></div>
        {grants.isLoading && <p className="p-4 text-xs text-muted-foreground">{t('common.loading')}</p>}
        {grants.error && <p className="p-4 text-xs text-danger">{String(grants.error)}</p>}
        {grants.data?.length === 0 && <p className="p-5 text-center text-xs text-muted-foreground">{t('resourceGrants.empty')}</p>}
        {grants.data?.map((grant) => <div className="grid min-h-12 grid-cols-[180px_minmax(0,1fr)_120px_48px] items-center border-t border-border px-4 text-xs" key={grant.id}><span>{grant.subjectType === 'department' ? t('resourceGrants.department') : t('resourceGrants.workflow')}</span><span className="truncate font-medium">{subjectNames.get(grant.subjectId) ?? grant.subjectId}</span><Badge className="w-fit" tone="primary">{t(operationLabelKeys[grant.operation as GrantOperation] ?? 'resourceGrants.operation')}</Badge>{auth.hasPermission('resource:grant') && <Button aria-label={t('resourceGrants.revoke')} onClick={() => setRevokeGrant(grant)} size="icon" variant="ghost"><Trash2 className="size-3.5" /></Button>}</div>)}
      </div>
      <div className="mt-5 flex justify-end"><Button onClick={onClose} variant="secondary">{t('common.cancel')}</Button></div>
    </DialogContent>
  </Dialog>
    <Dialog onOpenChange={(value) => { if (!value) setRevokeGrant(undefined) }} open={Boolean(revokeGrant)}>
      <DialogContent description={t('resourceGrants.revokeDescription')} title={t('resourceGrants.revokeTitle')}>
        <div className="p-5"><h2 className="text-sm font-semibold">{t('resourceGrants.revokeTitle')}</h2><p className="mt-2 text-xs leading-5 text-muted-foreground">{t('resourceGrants.revokeDescription')}</p><div className="mt-5 flex justify-end gap-2"><Button onClick={() => setRevokeGrant(undefined)} variant="ghost">{t('common.cancel')}</Button><Button disabled={remove.isPending} onClick={() => revokeGrant && remove.mutate(revokeGrant)} variant="danger">{t('resourceGrants.revokeConfirm')}</Button></div></div>
      </DialogContent>
    </Dialog>
  </>
}

function Field({ children, label }: { children: ReactNode; label: string }) { return <label className="block text-xs"><span className="mb-2 block font-medium">{label}<span aria-hidden="true" className="ml-1 text-danger">*</span></span>{children}</label> }
function operationsFor(type: string): GrantOperation[] { return type === 'rag' ? ['read', 'write', 'view', 'manage'] : ['use', 'view', 'manage'] }
