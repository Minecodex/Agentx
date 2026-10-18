import { useAuth } from '../../app/providers/auth-provider'
import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigate } from 'react-router-dom'

import { apiRequest } from '../../shared/api/client'
import type { components } from '../../shared/api/generated'
import { EndUserLabel, UserSourceFilter } from '../../shared/components/end-user-label'
import { StatusBadge } from '../../shared/components/status-badge'
import { localizedValue } from '../../shared/lib/localized-value'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import { Button } from '../../shared/ui/button'
import { Input } from '../../shared/ui/input'
import { Table, TableCell, TableContainer, TableHead } from '../../shared/ui/table'

export function ApplicationInvocations({ applicationId }: { applicationId: string }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const auth = useAuth()
  const { formatDateTime } = useLocaleFormat()
  const [user, setUser] = useState('')
  const [source, setSource] = useState('')
  const [cursors, setCursors] = useState<Array<string | undefined>>([undefined])
  const cursor = cursors[cursors.length - 1]
  const params = new URLSearchParams({ ...(user ? { externalUserId: user } : {}), ...(source ? { userSource: source } : {}), ...(cursor ? { cursor } : {}) })
  const records = useQuery({ queryKey: ['application-invocations', applicationId, params.toString()], queryFn: () => apiRequest<components['schemas']['ApplicationInvocationPageResponse']>(`/applications/${applicationId}/invocations?${params}`) })
  return <div className="rounded-lg border border-border bg-surface"><div className="flex items-center gap-3 border-b border-border p-4"><Input aria-label={t('applications.userFilter')} className="max-w-xs" placeholder={t('applications.userFilter')} value={user} onChange={event => { setUser(event.target.value); setCursors([undefined]) }} /><UserSourceFilter value={source} onChange={value => { setSource(value); setCursors([undefined]) }} /></div>{records.isError ? <p className="p-4 text-xs text-danger">{records.error.message}</p> : records.isLoading ? <p className="p-4 text-xs text-muted-foreground">{t('common.loading')}</p> : <TableContainer><Table><thead><tr><TableHead>ID</TableHead><TableHead>{t('applications.endUser')}</TableHead><TableHead>{t('common.status')}</TableHead><TableHead>{t('executions.startedAt')}</TableHead><TableHead /></tr></thead><tbody>{records.data?.items.map(item => <tr className="border-t border-border" key={item.invocationId}><TableCell className="font-mono text-[10px]">{item.invocationId}</TableCell><TableCell><EndUserLabel user={item.endUser} /></TableCell><TableCell><StatusBadge label={localizedValue(t, 'common', item.status)} status={item.status === 'completed' ? 'completed' : item.status === 'failed' ? 'failed' : 'running'} /></TableCell><TableCell>{formatDateTime(item.createdAt)}</TableCell><TableCell>{item.executionId && auth.hasPermission('dataset:manage') && <Button size="sm" variant="ghost" onClick={() => navigate(`/applications/${applicationId}?tab=testing&testTab=datasets&invocationId=${item.invocationId}`)}>{t('testing.addFromInvocation')}</Button>}{item.executionId && <Button asChild size="sm" variant="ghost"><Link to={`/executions/${item.executionId}`}>{t('executions.trace')}</Link></Button>}</TableCell></tr>)}{!records.data?.items.length && <tr><TableCell colSpan={5}>{t('common.noResults')}</TableCell></tr>}</tbody></Table></TableContainer>}<div className="flex justify-end gap-2 border-t border-border p-3"><Button disabled={cursors.length < 2} onClick={() => setCursors(values => values.slice(0, -1))} size="sm" variant="ghost">{t('common.previous')}</Button><Button disabled={!records.data?.next} onClick={() => setCursors(values => [...values, records.data?.next ?? undefined])} size="sm" variant="secondary">{t('common.next')}</Button></div></div>
}
