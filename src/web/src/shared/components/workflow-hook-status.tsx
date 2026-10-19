import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'

import { apiRequest } from '../api/client'
import type { DeliveryPage } from '../api/types'

export function WorkflowHookStatus({ applicationId, executionId, invocationId }: { applicationId: string; executionId: string; invocationId?: string | null }) {
  const { t } = useTranslation()
  const deliveries = useQuery({
    queryKey: ['workflow-hook-status', applicationId, invocationId ?? executionId],
    queryFn: () => apiRequest<DeliveryPage>(`/deliveries?applicationId=${applicationId}&${invocationId ? `invocationId=${invocationId}` : `executionId=${executionId}`}&limit=100`),
    refetchInterval: 5000,
  })
  const hooks = deliveries.data?.items.filter(item => item.provider === 'input_hook') ?? []
  if (!hooks.length) return null
  const statuses: Record<string, string> = {
    pending: t('applications.deliveryStatus_pending'), delivering: t('applications.deliveryStatus_delivering'),
    delivered: t('applications.deliveryStatus_delivered'), failed: t('applications.deliveryStatus_failed'),
    dead: t('applications.deliveryStatus_dead'), cancelled: t('common.cancelled'),
  }
  return <section className="mb-4 space-y-2" data-testid="workflow-hook-status"><h3 className="text-xs font-semibold">{t('interaction.notificationStatus')}</h3>{hooks.map(item => <div className="rounded-md border border-border p-3 text-xs" key={item.id}><div className="flex flex-wrap items-center gap-2"><code>{item.origin}</code><span>{statuses[item.status] ?? item.status}</span><span className="text-muted-foreground">{item.attemptCount} {t('applications.deliveryAttempts')}</span></div>{item.lastErrorCode && <p className="mt-1 break-words text-danger">{item.lastErrorCode}: {item.lastErrorMessage}</p>}</div>)}</section>
}
