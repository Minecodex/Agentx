import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useAuth } from '../../../app/providers/auth-provider'
import { apiRequest, jsonBody } from '../../../shared/api/client'
import { EntityFormDialog } from '../../../shared/components/entity-form-dialog'
import { ConfirmDialog } from '../../../shared/components/confirm-dialog'
import { Button } from '../../../shared/ui/button'
import { Card } from '../../../shared/ui/card'
import { useToast } from '../../../shared/ui/toast'
import { testingPath, type Profile } from './types'

export function EvaluationProfiles({ applicationId }: { applicationId: string }) {
  const { t } = useTranslation()
  const auth = useAuth()
  const client = useQueryClient()
  const { showToast } = useToast()
  const path = `${testingPath(applicationId)}/evaluation-profiles`
  const profiles = useQuery({ queryKey: ['application-profiles', applicationId], queryFn: () => apiRequest<Profile[]>(path) })
  const [editing, setEditing] = useState<Profile | 'new'>()
  const [removing, setRemoving] = useState<Profile>()
  const remove = useMutation({ mutationFn: () => apiRequest(`${path}/${removing!.id}?expectedVersion=${removing!.version}`, { method: 'DELETE' }), onSuccess: async () => { setRemoving(undefined); await client.invalidateQueries({ queryKey: ['application-profiles', applicationId] }) }, onError: (error: Error) => showToast(error.message) })
  const selected = editing && editing !== 'new' ? editing : undefined
  return <div className="space-y-4">
    <div className="flex items-center justify-between"><p className="text-xs text-muted-foreground">{t('testing.profileHint')}</p>{auth.hasPermission('evaluation_profile:manage') && <Button onClick={() => setEditing('new')}>{t('testing.createProfile')}</Button>}</div>
    {profiles.isLoading && <p>{t('common.loading')}</p>}{profiles.error && <p role="alert" className="text-danger">{profiles.error.message}</p>}
    {profiles.data?.length === 0 && <Card className="p-8 text-center text-sm text-muted-foreground">{t('testing.noProfiles')}</Card>}
    {profiles.data?.map(profile => <Card className="flex items-start gap-4 p-5" key={profile.id}><div className="min-w-0 flex-1"><h3 className="text-sm font-semibold">{profile.name} · v{profile.versionNumber}</h3><p className="mt-2 whitespace-pre-wrap text-xs text-muted-foreground">{profile.criteria}</p><p className="mt-3 text-xs">{t('testing.passThreshold')}: {Math.round(profile.passThreshold * 100)}</p></div><div className="flex gap-2">{auth.hasPermission('evaluation_profile:manage') && <Button variant="ghost" size="sm" onClick={() => setEditing(profile)}>{t('common.edit')}</Button>}{auth.hasPermission('evaluation_profile:delete') && <Button variant="ghost" size="sm" onClick={() => setRemoving(profile)}>{t('common.delete')}</Button>}</div></Card>)}
    {editing && <EntityFormDialog open title={selected ? t('testing.editProfile') : t('testing.createProfile')} cancelLabel={t('common.cancel')} submitLabel={t('common.save')} fields={[
      { name: 'name', label: t('common.name'), required: true, defaultValue: selected?.name ?? '' },
      { name: 'description', label: t('common.description'), type: 'textarea', defaultValue: selected?.description ?? '' },
      { name: 'criteria', label: t('testing.criteria'), type: 'textarea', required: true, defaultValue: selected?.criteria ?? t('testing.defaultCriteria') },
      { name: 'threshold', label: t('testing.passThreshold'), type: 'number', min: 0, max: 100, step: 1, required: true, defaultValue: String((selected?.passThreshold ?? 0.8) * 100) },
    ]} onClose={() => setEditing(undefined)} onSubmit={async values => { await apiRequest(`${path}${selected ? `/${selected.id}` : ''}`, { method: selected ? 'PATCH' : 'POST', body: jsonBody({ name: values.name, description: values.description || null, criteria: values.criteria, passThreshold: Number(values.threshold) / 100, version: selected?.version ?? null }) }); await client.invalidateQueries({ queryKey: ['application-profiles', applicationId] }); showToast(t('testing.saved')) }} />}
    <ConfirmDialog open={Boolean(removing)} title={t('common.delete')} description={t('testing.deleteProfileConfirm', { name: removing?.name })} cancelLabel={t('common.cancel')} confirmLabel={t('common.delete')} onClose={() => setRemoving(undefined)} onConfirm={() => remove.mutateAsync().then(() => undefined)} pending={remove.isPending} />
  </div>
}
