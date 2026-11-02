import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest, jsonBody } from '../../../shared/api/client'
import type { components } from '../../../shared/api/generated'
import { EntityFormDialog } from '../../../shared/components/entity-form-dialog'
import { Button } from '../../../shared/ui/button'
import { Card } from '../../../shared/ui/card'
import { useToast } from '../../../shared/ui/toast'

type Client = components['schemas']['ApplicationOAuthClient']
export function McpClients({ applicationId, publicationId, toolName, url }: { applicationId: string; publicationId: string; toolName: string; url: string }) {
  const { t } = useTranslation()
  const query = useQueryClient()
  const { showToast } = useToast()
  const path = `/applications/${applicationId}/mcp/clients`
  const clients = useQuery({ queryKey: ['application-oauth-clients', applicationId], queryFn: () => apiRequest<Client[]>(path) })
  const [register, setRegister] = useState(false)
  const [registrationId, setRegistrationId] = useState(() => crypto.randomUUID())
  const refresh = () => query.invalidateQueries({ queryKey: ['application-oauth-clients', applicationId] })
  const revoke = useMutation({ mutationFn: (id: string) => apiRequest(`${path}/${id}`, { method: 'DELETE' }), onSuccess: refresh })
  const matching = clients.data?.filter(client => client.resource === url) ?? []
  const active = matching.find(client => client.status === 'active')
  const configuration = { mcp: { [toolName]: { type: 'remote', url, oauth: active ? { clientId: active.clientId, scope: active.scope } : true } } }
  const text = JSON.stringify(configuration, null, 2)
  return <Card className="space-y-4 p-5">
    <div className="flex items-center justify-between gap-6"><h3 className="text-sm font-semibold">{t('applicationMcp.connect')}</h3><Button size="sm" variant="secondary" onClick={async () => { await navigator.clipboard.writeText(text); showToast(t('applicationMcp.copied')) }}>{t('applicationMcp.copy')}</Button></div>
    <pre className="overflow-auto rounded-lg bg-canvas p-4 text-xs leading-6">{text}</pre>
    <p className="text-xs text-muted-foreground">{t('applicationMcp.keyHint')}</p>
    <details className="border-t border-border pt-4">
      <summary className="cursor-pointer text-xs font-semibold">{t('applicationMcp.clientTitle')}</summary>
      <div className="mt-4 space-y-4"><div className="flex items-start justify-between gap-6"><p className="text-xs leading-5 text-muted-foreground">{t('applicationMcp.clientDescription')}</p><Button size="sm" variant="secondary" onClick={() => { setRegistrationId(crypto.randomUUID()); setRegister(true) }}>{t('applicationMcp.register')}</Button></div>
        {matching.map(client => <div key={client.clientId} className="flex items-center gap-4 rounded-lg border border-border p-3 text-xs"><div className="min-w-0 flex-1"><strong>{client.name}</strong><p className="mt-1 break-all text-muted-foreground">{t('applicationMcp.registrationId')}: {client.clientId}</p><p className="mt-1 text-muted-foreground">{client.redirectUris.join(' · ')}</p></div><span>{client.status}</span>{client.status !== 'revoked' && <Button disabled={revoke.isPending} size="sm" variant="secondary" onClick={() => revoke.mutate(client.clientId)}>{t('applicationMcp.revoke')}</Button>}</div>)}
        <a className="text-xs text-primary underline" href="/oauth/grants" target="_blank" rel="noreferrer">{t('applicationMcp.myGrants')}</a>
      </div>
    </details>
    {(clients.error || revoke.error) && <p className="text-xs text-danger" role="alert">{clients.error?.message ?? revoke.error?.message}</p>}
    {register && <EntityFormDialog open title={t('applicationMcp.clientTitle')} submitLabel={t('applicationMcp.register')} cancelLabel={t('common.cancel')} fields={[{ name: 'name', label: t('applicationMcp.clientName'), required: true }, { name: 'callbacks', label: t('applicationMcp.callbacks'), type: 'textarea', required: true, defaultValue: 'http://127.0.0.1:19876/mcp/oauth/callback' }]} onClose={() => setRegister(false)} onSubmit={async values => {
      await apiRequest(path, { method: 'POST', body: jsonBody({ requestId: registrationId, publicationId, name: values.name, redirectUris: values.callbacks.split('\n').map(value => value.trim()).filter(Boolean) }) })
      await refresh()
    }} />}
  </Card>
}
