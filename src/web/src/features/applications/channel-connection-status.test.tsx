import { render, screen } from '@testing-library/react'
import { I18nextProvider } from 'react-i18next'
import { describe, expect, it } from 'vitest'

import { i18n } from '../../app/i18n'
import { ChannelConnectionStatus } from './channel-connection-status'

function show(published = false, connectionStatus?: string | null, status = 'active', loading = false) {
  render(<I18nextProvider i18n={i18n}><ChannelConnectionStatus channel={{ status, connectionStatus }} loading={loading} published={published} publishAction={<button>Publish</button>} /></I18nextProvider>)
}

describe('channel publication and connection state', () => {
  it('explains that an unpublished channel has not started connecting', () => {
    show()
    expect(screen.getByText(i18n.t('applications.pendingPublish'))).toBeVisible()
    expect(screen.queryByText(i18n.t('applications.connectionPending'))).toBeNull()
    expect(screen.getByText(i18n.t('applications.streamPublishRequired'))).toBeVisible()
    expect(screen.getByRole('button', { name: 'Publish' })).toBeVisible()
  })

  it('does not mistake unavailable runtime status for a pending connection', () => {
    show(true)
    expect(screen.getByText(i18n.t('applications.connectionUnavailable'))).toBeVisible()
    expect(screen.queryByText(i18n.t('applications.connectionPending'))).toBeNull()
    expect(screen.queryByRole('button')).toBeNull()
  })

  it.each(['pending', 'connected', 'reconnecting', 'disconnected', 'error'])('shows the actual %s runtime state after publication', (status) => {
    show(true, status)
    const labels: Record<string, string> = { pending: 'connectionPending', connected: 'connectionConnected', reconnecting: 'connectionReconnecting', disconnected: 'connectionDisconnected', error: 'connectionError' }
    expect(screen.getByText(i18n.t(`applications.${labels[status]}`))).toBeVisible()
    expect(screen.queryByRole('button')).toBeNull()
  })

  it('retains the active connection while explaining unpublished configuration changes', () => {
    show(false, 'connected')
    expect(screen.getByText(i18n.t('applications.connectionConnected'))).toBeVisible()
    expect(screen.getByText(i18n.t('applications.streamConfigPending'))).toBeVisible()
    expect(screen.getByRole('button', { name: 'Publish' })).toBeVisible()
  })

  it('explains a disabled channel without prompting publication', () => {
    show(false, null, 'disabled')
    expect(screen.getByText(i18n.t('applications.channelDisabled'))).toBeVisible()
    expect(screen.queryByRole('button')).toBeNull()
  })

  it('waits for deployment metadata before diagnosing publication', () => {
    show(false, null, 'active', true)
    expect(screen.getByText(i18n.t('common.loading'))).toBeVisible()
    expect(screen.queryByText(i18n.t('applications.pendingPublish'))).toBeNull()
    expect(screen.queryByRole('button')).toBeNull()
  })
})
