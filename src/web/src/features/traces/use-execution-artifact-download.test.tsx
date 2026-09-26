import { act, renderHook, waitFor } from '@testing-library/react'
import type { PropsWithChildren } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ToastProvider } from '../../shared/ui/toast'
import { useExecutionArtifactDownload } from './use-execution-artifact-download'

describe('Execution Trace Artifact download', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('downloads through the execution-scoped authorized endpoint', async () => {
    const fetchMock = vi.fn(async () => new Response('{"trace":"large"}', { headers: { 'Content-Type': 'application/json' } }))
    vi.stubGlobal('fetch', fetchMock)
    const nativeUrl = URL
    class DownloadUrl extends nativeUrl {
      static createObjectURL = vi.fn(() => 'blob:trace-artifact')
      static revokeObjectURL = vi.fn()
    }
    vi.stubGlobal('URL', DownloadUrl)
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => undefined)
    const wrapper = ({ children }: PropsWithChildren) => <ToastProvider>{children}</ToastProvider>
    const { result } = renderHook(() => useExecutionArtifactDownload('execution-1'), { wrapper })

    await act(() => result.current('artifact-1'))

    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith(
      '/api/v1/executions/execution-1/artifacts/artifact-1',
      expect.objectContaining({ headers: expect.any(Headers) }),
    ))
    expect(DownloadUrl.createObjectURL).toHaveBeenCalledTimes(1)
    const blobArg = (DownloadUrl.createObjectURL as ReturnType<typeof vi.fn>).mock.calls[0]?.[0] as Blob | undefined
    expect(blobArg?.size).toBe('{"trace":"large"}'.length)
    expect(blobArg?.type).toBe('application/json')
    expect(click).toHaveBeenCalledOnce()
    expect(DownloadUrl.revokeObjectURL).toHaveBeenCalledWith('blob:trace-artifact')
    click.mockRestore()
  })
})
