import { beforeEach, describe, expect, it, vi } from 'vitest'

import { apiRequest } from '../../../shared/api/client'
import { loadResourceOptions } from './use-resource-options'

vi.mock('../../../shared/api/client', () => ({ apiRequest: vi.fn() }))

describe('loadResourceOptions', () => {
  beforeEach(() => vi.mocked(apiRequest).mockReset())

  it('uses the declared operation and follows server pagination until every visible resource is loaded', async () => {
    vi.mocked(apiRequest)
      .mockResolvedValueOnce({
        items: Array.from({ length: 100 }, (_, index) => ({ id: `rag-${index}`, name: `RAG ${index}`, detail: '', status: 'active', accessState: 'authorized' })),
        page: 1,
        pageSize: 100,
        total: 101,
      })
      .mockResolvedValueOnce({
        items: [{ id: 'rag-100', name: 'RAG 100', detail: '', status: 'active', accessState: 'requestable' }],
        page: 2,
        pageSize: 100,
        total: 101,
      })

    const options = await loadResourceOptions('workflow-1', 'draft-1', { resourceType: 'rag', operation: 'read' })

    expect(apiRequest).toHaveBeenNthCalledWith(1, '/workflows/workflow-1/resource-options?draftId=draft-1&resourceType=rag&operation=read&page=1&pageSize=100')
    expect(apiRequest).toHaveBeenNthCalledWith(2, '/workflows/workflow-1/resource-options?draftId=draft-1&resourceType=rag&operation=read&page=2&pageSize=100')
    expect(options).toHaveLength(101)
    expect(options.at(-1)).toMatchObject({ value: 'rag-100', resourceType: 'rag', operation: 'read' })
  })

  it('keeps model metadata so budget defaults can follow the selected model', async () => {
    vi.mocked(apiRequest).mockResolvedValueOnce({
      items: [{
        id: 'model-1',
        name: 'GLM-4.7',
        detail: 'conn / glm-4.7',
        status: 'active',
        accessState: 'authorized',
        metadata: { maxInputTokens: 200000, maxOutputTokens: 32000, currency: 'CNY' },
      }],
      page: 1,
      pageSize: 100,
      total: 1,
    })

    const options = await loadResourceOptions('workflow-1', 'draft-1', { resourceType: 'model', operation: 'use' })

    expect(options[0]).toMatchObject({
      value: 'model-1',
      metadata: { maxInputTokens: 200000, maxOutputTokens: 32000, currency: 'CNY' },
    })
  })
})
