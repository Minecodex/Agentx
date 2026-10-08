import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import type { ApplicationWebhook } from '../../shared/api/types'
import { StatusBadge, type StatusValue } from '../../shared/components/status-badge'

type Props = {
  channel: Pick<ApplicationWebhook, 'status' | 'connectionStatus' | 'connectionError' | 'lastConnectedAt'>
  loading: boolean
  published: boolean
  publishAction?: ReactNode
}

const connectionStates: Record<string, { label: string; tone: StatusValue }> = {
  pending: { label: 'connectionPending', tone: 'pending' },
  connected: { label: 'connectionConnected', tone: 'active' },
  reconnecting: { label: 'connectionReconnecting', tone: 'running' },
  disconnected: { label: 'connectionDisconnected', tone: 'inactive' },
  error: { label: 'connectionError', tone: 'failed' },
}

export function ChannelConnectionStatus({ channel, loading, published, publishAction }: Props) {
  const { t } = useTranslation()
  const status = channel.connectionStatus
  const disabled = !status && channel.status !== 'active'
  const needsPublication = !loading && !published && !disabled
  const state = status ? connectionStates[status] : undefined
  const label = loading ? t('common.loading')
    : disabled ? t('applications.channelDisabled')
      : state ? t(`applications.${state.label}`)
        : needsPublication ? t('applications.pendingPublish') : t('applications.connectionUnavailable')
  const tone: StatusValue = loading ? 'pending' : disabled ? 'inactive'
    : state?.tone ?? (needsPublication ? 'pending' : 'inactive')
  const hint = disabled ? t('applications.streamDisabledHint')
    : needsPublication ? t(status ? 'applications.streamConfigPending' : 'applications.streamPublishRequired')
      : !state ? t('applications.connectionUnavailableHint') : t('applications.streamNoEndpoint')

  return <div aria-label={t('applications.connectionStatus')} className="space-y-4" role="group">
    <p className="text-xs text-muted-foreground">{t('applications.connectionStatus')}</p>
    <div className="flex items-center gap-3">
      <StatusBadge label={label} status={tone} />
      {!loading && status && channel.lastConnectedAt && <span className="text-[11px] text-muted-foreground">{channel.lastConnectedAt}</span>}
    </div>
    {!loading && status && channel.connectionError && <p className="rounded-md border border-danger/30 bg-danger/5 p-3 text-[11px] text-danger">{channel.connectionError}</p>}
    {!loading && <div className="rounded-md border border-dashed border-border p-3 text-xs text-muted-foreground">
      <p>{hint}</p>
      {needsPublication && publishAction && <div className="mt-3">{publishAction}</div>}
    </div>}
  </div>
}
