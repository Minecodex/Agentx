import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook, waitFor } from '@testing-library/react'
import type { PropsWithChildren } from 'react'
import { afterEach, expect, it, vi } from 'vitest'

import { apiRequest } from '../../../shared/api/client'
import type { NodeManifest, StudioDocument, StudioNode } from '../model/types'
import { useResolvedPluginManifests } from './use-resolved-plugin-manifests'

vi.mock('../../../shared/api/client', () => ({ apiRequest: vi.fn(), jsonBody: JSON.stringify }))
afterEach(() => vi.clearAllMocks())

const manifest = {
  nodeType: 'acme.chain', version: 1, capability: 'plugin_nodejs',
  plugin: { packageId: 'acme/chain', bundleDigest: 'sha256:test' },
  outputPorts: [{ name: 'main', kind: 'main' }], outputSchema: { properties: { oldField: { type: 'string' } } },
} as NodeManifest
const manifests = new Map([['acme.chain@1', manifest]])
const node = (id: string): StudioNode => ({
  id, type: 'manifest', position: { x: 0, y: 0 },
  data: { editorKind: 'action', nodeType: 'acme.chain', typeVersion: 1, label: id, key: id,
    disabled: false, parameters: { field: 'first' }, resourceReferences: [], contextWrites: [], settings: {} },
})
const document = (): StudioDocument => ({
  start: { inputs: { type: 'object', properties: { input: { type: 'string' } } }, contexts: {} },
  nodes: [node('a'), node('b')], edges: [
    { id: 'start-a', source: '__start__', target: 'a', sourceHandle: 'main', targetHandle: 'main' },
    { id: 'a-b', source: 'a', target: 'b', sourceHandle: 'main', targetHandle: 'main' },
  ], end: { completion: 'first_return', outputs: {}, error: { outputs: {} } },
  settings: { executionOrder: 'deterministic', activationBudget: 10000 },
  viewport: { x: 0, y: 0, zoom: 1 }, boundaryLayouts: [], annotations: [], groups: [],
})
const setup = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  return ({ children }: PropsWithChildren) => <QueryClientProvider client={client}>{children}</QueryClientProvider>
}

it('resolves the current graph and invalidates downstream contracts when upstream inputs change', async () => {
  vi.mocked(apiRequest).mockImplementation(async (_path, options) => {
    const { definition } = JSON.parse(options!.body as string)
    const field = definition.nodes[0].parameters.field
    return { nodes: Object.fromEntries(['a', 'b'].map(id => [id, { status: 'complete', outputSchema: { properties: { [field]: { type: 'number' } } } }])) }
  })
  const initial = document()
  const { result, rerender } = renderHook(({ value }) => useResolvedPluginManifests('workflow', 'draft', value, manifests), {
    initialProps: { value: initial }, wrapper: setup(),
  })
  await waitFor(() => expect(result.current.resolved.get('b')?.outputSchema).toEqual({ properties: { first: { type: 'number' } } }))
  const request = vi.mocked(apiRequest).mock.calls[0]
  expect(request[0]).toBe('/workflows/workflow/drafts/draft/resolve-plugins')
  expect(JSON.parse(request[1]!.body as string).definition.start).toEqual(initial.start)
  const changed = structuredClone(initial)
  changed.nodes[0].data.parameters = { field: 'second' }
  rerender({ value: changed })
  expect(result.current.resolved.has('b')).toBe(false)
  await waitFor(() => expect(result.current.resolved.get('b')?.outputSchema).toEqual({ properties: { second: { type: 'number' } } }))
  expect(apiRequest).toHaveBeenCalledTimes(2)
})

it('does not apply late responses from an older graph', async () => {
  let finishOld: (value: unknown) => void = () => undefined
  vi.mocked(apiRequest).mockImplementationOnce(() => new Promise(resolve => { finishOld = resolve }))
    .mockResolvedValue({ nodes: { b: { status: 'complete', outputSchema: { title: 'current' } } } })
  const initial = document()
  const { result, rerender } = renderHook(({ value }) => useResolvedPluginManifests('workflow', 'draft', value, manifests), {
    initialProps: { value: initial }, wrapper: setup(),
  })
  await waitFor(() => expect(apiRequest).toHaveBeenCalledTimes(1))
  const changed = structuredClone(initial)
  changed.start.inputs = { type: 'object', properties: { updated: { type: 'string' } } }
  rerender({ value: changed })
  await waitFor(() => expect(result.current.resolved.get('b')?.outputSchema).toEqual({ title: 'current' }))
  await act(async () => finishOld({ nodes: { b: { status: 'complete', outputSchema: { title: 'old' } } } }))
  expect(result.current.resolved.get('b')?.outputSchema).toEqual({ title: 'current' })
})

it('keeps node contracts available while the shared end contract is edited', async () => {
  vi.mocked(apiRequest).mockResolvedValue({ nodes: { b: { status: 'complete', outputSchema: { title: 'resolved' } } } })
  const initial = document()
  const { result, rerender } = renderHook(({ value }) => useResolvedPluginManifests('workflow', 'draft', value, manifests), {
    initialProps: { value: initial }, wrapper: setup(),
  })
  await waitFor(() => expect(result.current.resolved.has('b')).toBe(true))
  rerender({ value: { ...initial, end: { ...initial.end, outputs: { answer: { schema: { type: 'number' }, required: false, sensitive: false } } } } })
  expect(result.current.resolved.get('b')?.outputSchema).toEqual({ title: 'resolved' })
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 200)) })
  expect(apiRequest).toHaveBeenCalledTimes(1)
})
