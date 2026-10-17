import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { apiRequest, jsonBody } from '../../shared/api/client'
import type { User, WorkflowMember } from '../../shared/api/types'
import { Dialog, DialogContent } from '../../shared/ui/dialog'
import { Button } from '../../shared/ui/button'
import { Input } from '../../shared/ui/input'
import { Select } from '../../shared/ui/select'
import { useToast } from '../../shared/ui/toast'

export function WorkflowMembersDialog({ workflowId, canManage, ownerId, onClose }: { workflowId: string; canManage: boolean; ownerId: string; onClose: () => void }) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const [userId, setUserId] = useState('')
  const [role, setRole] = useState('viewer')
  const [search, setSearch] = useState('')
  const members = useQuery({ queryKey: ['workflow-members', workflowId], queryFn: () => apiRequest<WorkflowMember[]>(`/workflows/${workflowId}/members`) })
  const users = useQuery({ enabled: canManage, queryKey: ['workflow-member-candidates', workflowId, search], queryFn: () => apiRequest<Array<Pick<User, 'id' | 'username' | 'displayName'>>>(`/workflows/${workflowId}/member-candidates?search=${encodeURIComponent(search)}`) })
  const roles = [{value:'viewer',label:t('workflows.memberRoles.viewer')},{value:'editor',label:t('workflows.memberRoles.editor')},{value:'manager',label:t('workflows.memberRoles.manager')}]
  const change = useMutation({
    mutationFn: ({ id, memberRole }: { id: string; memberRole?: string }) => apiRequest(`/workflows/${workflowId}/members${memberRole ? '' : `/${id}`}`, { method: memberRole ? 'POST' : 'DELETE', ...(memberRole ? { body: jsonBody({ userId: id, memberRole }) } : {}) }),
    onSuccess: async () => { setUserId(''); await Promise.all([queryClient.invalidateQueries({ queryKey: ['workflow-members', workflowId] }), queryClient.invalidateQueries({ queryKey: ['workflow-member-candidates', workflowId] })]) },
    onError: (error: Error) => showToast(error.message),
  })
  return <Dialog open onOpenChange={open => { if (!open) onClose() }}><DialogContent title={t('workflows.versionCanvas.manageMembers')}>
    <div className="border-b border-border px-6 py-5"><h2 className="text-lg font-semibold">{t('workflows.versionCanvas.manageMembers')}</h2></div>
    <div className="max-h-[60vh] space-y-4 overflow-y-auto p-6">
      {members.isLoading && <p className="text-xs text-muted-foreground">{t('common.loading')}</p>}
      {members.error && <p role="alert" className="text-xs text-danger">{members.error.message}</p>}
      {members.data?.map(member => <div key={member.userId} className="flex items-center gap-3 border-b border-border pb-3"><div className="min-w-0 flex-1 text-xs"><p className="font-medium">{member.displayName}</p><p className="mt-1 text-muted-foreground">{member.username}</p></div>{canManage && member.userId !== ownerId ? <><Select aria-label={t('workflows.versionCanvas.memberRole', { name: member.displayName })} disabled={change.isPending} value={member.memberRole} options={roles} onValueChange={memberRole => change.mutate({ id: member.userId, memberRole })} /><Button size="sm" variant="ghost" disabled={change.isPending} onClick={() => change.mutate({ id: member.userId })}>{t('workflows.versionCanvas.removeMember')}</Button></> : <span className="text-xs text-muted-foreground">{t(`workflows.memberRoles.${member.userId === ownerId ? 'owner' : member.memberRole}`)}</span>}</div>)}
      {canManage && <div className="space-y-3">{users.error && <p role="alert" className="text-xs text-danger">{users.error.message}</p>}<Input aria-label={t('workflows.versionCanvas.searchMember')} placeholder={t('workflows.versionCanvas.searchMember')} value={search} onChange={event => setSearch(event.target.value)} /><div className="flex gap-2"><Select aria-label={t('workflows.addMember')} className="min-w-0 flex-1" value={userId} onValueChange={setUserId} options={(users.data ?? []).filter(user => !members.data?.some(member => member.userId === user.id)).map(user => ({ value: user.id, label: `${user.displayName} · ${user.username}` }))} /><Select aria-label={t('workflows.versionCanvas.newMemberRole')} value={role} onValueChange={setRole} options={roles} /><Button disabled={!userId || change.isPending} onClick={() => change.mutate({ id: userId, memberRole: role })}>{t('workflows.addMember')}</Button></div></div>}
    </div><div className="flex justify-end border-t border-border px-6 py-4"><Button variant="secondary" onClick={onClose}>{t('common.close')}</Button></div>
  </DialogContent></Dialog>
}
