import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { apiRequest } from '../../../shared/api/client'
import { DesignEffectsContext } from '../model/design-effects'
import type { NodeManifest } from '../model/types'
import { useProviderOptions } from './use-provider-options'

vi.mock('../../../shared/api/client', () => ({ apiRequest: vi.fn() }))

function providerManifest(nodeType: string, provider: string, capability = 'builtin'): NodeManifest {
  return {
    protocolVersion: '3.0', nodeType, version: 1, capability, displayName: nodeType,
    description: '', category: 'actions', keywords: [], iconKey: 'box', executionStyle: 'action',
    readiness: 'any', inputPorts: [], outputPorts: [], bindingSlots: [], parameterSchema: {},
    providers: [provider], credentials: [], sandboxRequired: false, supportsMock: true, sideEffectLevel: 'none',
    retryPolicy: { retryable: false, maxAttempts: 1, initialBackoffMs: 0, maxBackoffMs: 0 },
    uiSchema: { fields: { target: { control: 'provider_options', provider } } },
  }
}

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return <QueryClientProvider client={client}><DesignEffectsContext.Provider value>{children}</DesignEffectsContext.Provider></QueryClientProvider>
}

describe('design-only provider metadata', () => {
  beforeEach(() => {
    vi.mocked(apiRequest).mockReset()
    vi.mocked(apiRequest).mockResolvedValue({ items: [{ value: 'id', label: 'Available metadata' }] })
  })

  it.each([
    ['approval', 'users'],
    ['sub_workflow', 'workflow_versions'],
  ])('loads Control-owned metadata for %s', async (nodeType, provider) => {
    const { result } = renderHook(() => useProviderOptions(providerManifest(nodeType, provider)), { wrapper })
    await waitFor(() => expect(result.current.target).toEqual([{ value: 'id', label: 'Available metadata', detail: undefined, manifest: undefined }]))
    expect(apiRequest).toHaveBeenCalledWith(expect.stringContaining(`/providers/${provider}?`), expect.any(Object))
  })

  it('does not invoke a plugin provider even when it uses a metadata provider name', async () => {
    const { result } = renderHook(() => useProviderOptions(providerManifest('acme.lookup', 'users', 'plugin_nodejs')), { wrapper })
    await waitFor(() => expect(result.current.target).toEqual([]))
    expect(apiRequest).not.toHaveBeenCalled()
  })
})
