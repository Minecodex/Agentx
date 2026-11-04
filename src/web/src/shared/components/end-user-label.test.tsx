import { render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'

import { i18n } from '../../app/i18n'
import { EndUserLabel } from './end-user-label'

const userId = '01a13556-0d5c-7972-b70b-a045766b2171'

describe('end user labels', () => {
  beforeEach(async () => { await i18n.changeLanguage('zh-CN') })

  it('shows the platform name and keeps the full ID in the hover description', () => {
    render(<EndUserLabel user={{ source: 'platform', externalUserId: userId, displayName: '调用用户张三' }} />)
    expect(screen.getByText('调用用户张三')).toBeInTheDocument()
    expect(screen.queryByText(userId)).not.toBeInTheDocument()
    expect(screen.getByText('调用用户张三').parentElement).toHaveAttribute('title', `调用用户张三\n${userId}`)
    expect(screen.getByText('平台登录')).toBeInTheDocument()
  })

  it.each(['api_parameter', 'channel'] as const)('keeps %s declarations separate from platform names', (source) => {
    render(<EndUserLabel user={{ source, externalUserId: userId, displayName: '同 ID 的平台用户' }} />)
    expect(screen.getByText(userId)).toBeInTheDocument()
    expect(screen.queryByText('同 ID 的平台用户')).not.toBeInTheDocument()
  })

  it('identifies an unavailable platform user without filling the cell with a UUID', () => {
    render(<EndUserLabel user={{ source: 'platform', externalUserId: userId, displayName: null }} />)
    expect(screen.getByText('用户已删除或不可用')).toBeInTheDocument()
    expect(screen.queryByText(userId)).not.toBeInTheDocument()
  })

  it('localizes unavailable users in English', async () => {
    await i18n.changeLanguage('en-US')
    render(<EndUserLabel user={{ source: 'platform', externalUserId: userId, displayName: null }} />)
    expect(screen.getByText('Deleted or unavailable user')).toBeInTheDocument()
    expect(screen.getByText('Platform login')).toBeInTheDocument()
  })

  it('shows a dash when the invocation has no user identity', () => {
    render(<EndUserLabel user={null} />)
    expect(screen.getByText('—')).toHaveAttribute('title', '未提供用户标识，个人长期记忆不可用。')
  })
})
