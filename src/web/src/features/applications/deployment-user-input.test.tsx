import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../../app/i18n'
import { DeploymentUserInput } from './deployment-user-input'

const definition = {
  start: { inputs: { type: 'object', properties: {
    customerId: { type: 'string' }, count: { type: 'integer' },
    profile: { type: 'object', properties: { id: { type: 'string' } } },
  } } },
  end: { outputs: {} },
}

describe('deployment API user mapping', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('zh-CN')
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ definition, versionNumber: 3 }), { headers: { 'Content-Type': 'application/json' } })))
  })

  it('starts collapsed and offers only top-level string fields with a no-mapping choice', async () => {
    const onChange = vi.fn()
    const { container } = renderField('', onChange)
    expect(container.querySelector('details')).not.toHaveAttribute('open')
    fireEvent.click(screen.getByText('高级配置'))
    const field = await screen.findByRole('combobox')
    await waitFor(() => expect(field).not.toBeDisabled())
    fireEvent.click(field)
    expect(screen.getByRole('option', { name: '不设置' })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('option', { name: 'customerId' }))
    expect(onChange).toHaveBeenCalledWith('customerId')
    expect(screen.queryByRole('option', { name: 'count' })).not.toBeInTheDocument()
    expect(screen.queryByRole('option', { name: 'profile' })).not.toBeInTheDocument()
    expect(screen.getByText(/不验证用户身份/)).toBeInTheDocument()
    expect(screen.getByText(/长期记忆/)).toBeInTheDocument()
  })

  it('drops a previous mapping when the selected published version no longer contains it', async () => {
    const onChange = vi.fn()
    renderField('removedUserField', onChange)
    await waitFor(() => expect(onChange).toHaveBeenCalledWith(''))
  })
})

function renderField(value: string, onChange: (value: string) => void) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(<QueryClientProvider client={client}><DeploymentUserInput workflowId="workflow-1" versionId="version-3" value={value} onChange={onChange} /></QueryClientProvider>)
}
