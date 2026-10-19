import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen } from '@testing-library/react'
import { expect, it, vi } from 'vitest'

import { i18n } from '../../app/i18n'
import { apiRequest } from '../api/client'
import { WorkflowHookStatus } from './workflow-hook-status'

vi.mock('../api/client', () => ({ apiRequest: vi.fn() }))

it('shows terminal Hook failure while retaining the invocation scope', async () => {
  await i18n.changeLanguage('en-US')
  vi.mocked(apiRequest).mockResolvedValue({ items: [{ id: 'notice', provider: 'input_hook', origin: 'wait:wait-1', status: 'dead', attemptCount: 1, lastErrorCode: 'WAIT_HOOK_REJECTED', lastErrorMessage: 'Hook returned HTTP 404' }] })
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(<QueryClientProvider client={client}><WorkflowHookStatus applicationId="app-1" executionId="execution-1" invocationId="invocation-1" /></QueryClientProvider>)
  expect(await screen.findByText('WAIT_HOOK_REJECTED: Hook returned HTTP 404')).toBeVisible()
  expect(apiRequest).toHaveBeenCalledWith('/deliveries?applicationId=app-1&invocationId=invocation-1&limit=100')
  client.clear()
})
