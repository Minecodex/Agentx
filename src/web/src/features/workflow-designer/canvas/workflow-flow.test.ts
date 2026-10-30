import { canvasEdgePortKinds } from '../utils/edge-port-kinds'
import { computeGroupView } from '../utils/group-geometry'
import { describe, expect, it } from 'vitest'

import type { StudioEdge } from '../model/types'
import { studioManifest } from '../testing/studio-catalog'
import { proxyEdges } from '../utils/group-edges'
import { containerEscapeOverflow, findDropContainer, indexContainers, loopBoundaryEdges, materializeRuntimeEdges, readonlyContainerViews } from './workflow-flow'

describe('collapsed Group edge proxies', () => {
  it('hides internal edges and aggregates external handles onto the proxy node', () => {
    const edges: StudioEdge[] = [
      { id: 'internal', source: 'a', target: 'b', sourceHandle: 'main', targetHandle: 'main', data: { edgeKind: 'execution', order: 0 } },
      { id: 'outgoing', source: 'b', target: 'c', sourceHandle: 'main', targetHandle: 'main', data: { edgeKind: 'execution', order: 1 } },
      { id: 'incoming', source: 'd', target: 'a', sourceHandle: 'error', targetHandle: 'main', data: { edgeKind: 'execution', order: 0 } },
    ]
    const collapsed = new Map([['a', 'group:g'], ['b', 'group:g']])

    expect(proxyEdges(edges, collapsed)).toEqual([
      expect.objectContaining({ id: 'outgoing', source: 'group:g', sourceHandle: 'group-out', target: 'c' }),
      expect.objectContaining({ id: 'incoming', source: 'd', target: 'group:g', targetHandle: 'group-in' }),
    ])
  })

  it('reuses unrelated edge objects when one node runtime status changes', () => {
    const edges: StudioEdge[] = [
      { id: 'affected', source: 'a', target: 'b', data: { edgeKind: 'execution' } },
      { id: 'unrelated', source: 'c', target: 'd', data: { edgeKind: 'execution' } },
    ]
    const cache = new Map()
    const before = materializeRuntimeEdges(edges, new Map(), cache)
    const after = materializeRuntimeEdges(edges, new Map([['a', 'running']]), cache)
    expect(after[0]).not.toBe(before[0])
    expect(after[0].data?.runtimeStatus).toBe('running')
    expect(after[1]).toBe(before[1])
  })

  it('recovers custom plugin error kinds from contracts after save and refresh', () => {
    const source = { id: 'plugin', type: 'manifest' as const, position: { x: 0, y: 0 }, data: { editorKind: 'action' as const, nodeType: 'acme.custom', typeVersion: 1, label: 'Plugin', key: 'plugin', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false } }
    const manifest = { ...studioManifest('set'), nodeType: 'acme.custom', outputPorts: [{ name: 'failed', kind: 'error' as const, required: false, variadic: false }] }
    const edges: StudioEdge[] = [{ id: 'failed', source: 'plugin', sourceHandle: 'failed', target: 'exit', targetHandle: 'error', data: { edgeKind: 'execution' } }]
    const result = canvasEdgePortKinds(edges, [source], new Map([['acme.custom@1', manifest]]))
    expect(result[0].data?.sourcePortKind).toBe('error')
    expect(edges[0].data?.sourcePortKind).toBeUndefined()
    expect(proxyEdges(result, new Map([['plugin', 'group:g']]))[0].sourceHandle).toBe('group-error-out')
  })

  it('preserves a custom error input on collapsed targets and keeps derived kinds out of canonical edges', () => {
    const node = { id: 'plugin', type: 'manifest' as const, position: { x: 0, y: 0 }, data: { editorKind: 'action' as const, nodeType: 'acme.custom', typeVersion: 1, label: 'Plugin', key: 'plugin', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false } }
    const manifest = { ...studioManifest('set'), nodeType: 'acme.custom', inputPorts: [{ name: 'failure', kind: 'error' as const, required: false, variadic: false }] }
    const edges: StudioEdge[] = [{ id: 'failure', source: 'source', sourceHandle: 'error', target: node.id, targetHandle: 'failure', data: { edgeKind: 'execution' } }]
    const result = proxyEdges(canvasEdgePortKinds(edges, [node], new Map([['acme.custom@1', manifest]])), new Map([[node.id, 'group:g']]))
    expect(result[0].targetHandle).toBe('group-error-in')
    expect(result[0].data?.targetPortKind).toBe('error')
    expect(edges[0].data?.targetPortKind).toBeUndefined()
  })
})

describe('loop container view projection', () => {
  const loop = { id: 'loop-1', type: 'manifest' as const, position: { x: 600, y: 120 }, width: 470, height: 300, data: { editorKind: 'action' as const, nodeType: 'loop_over_items', typeVersion: 1, label: 'Loop', key: 'loop', parameters: { parallelism: 4 }, contextWrites: [], resourceReferences: [], settings: {}, disabled: false } }
  const body = { id: 'body-1', type: 'manifest' as const, position: { x: 40, y: 130 }, data: { editorKind: 'action' as const, nodeType: 'set', typeVersion: 1, label: 'Body', key: 'body', parentId: 'loop-1', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false } }
  const free = { id: 'free-1', type: 'manifest' as const, position: { x: 60, y: 40 }, data: { editorKind: 'action' as const, nodeType: 'set', typeVersion: 1, label: 'Free', key: 'free', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false } }

  it('expands read-only loop views around restored members without mutating their saved frame', () => {
    const compact = { ...loop, width: 848, height: 280 }
    const first = { ...body, position: { x: 336, y: 132 } }
    const second = { ...body, id: 'body-2', position: { x: 672, y: 132 } }
    const containers = indexContainers([compact, first, second])
    const manifests = new Map([['set@1', studioManifest('set')]])
    const expanded = readonlyContainerViews(containers, new Map(), [], manifests)
    expect(expanded.get(loop.id)!.loop.width).toBeGreaterThanOrEqual(1216)
    expect(compact.width).toBe(848)
    const hidden = new Map([[first.id, 'group:g'], [second.id, 'group:g']])
    const proxy = computeGroupView({ id: 'g', label: 'Group', nodeIds: [first.id, second.id], collapsed: true }, new Map([compact, first, second].map(node => [node.id, node])), manifests)!
    expect(proxy.position).toEqual({ x: 304, y: 84 })
    expect(readonlyContainerViews(containers, hidden, [proxy], manifests).get(loop.id)!.loop.width).toBe(848)
  })

  it('keeps empty loops in container mode and adds owned children', () => {
    const containers = indexContainers([loop, body, free])
    expect(containers.has('loop-1')).toBe(true)
    expect(containers.get('loop-1')?.childIds.has('body-1')).toBe(true)
    expect(indexContainers([loop, free]).size).toBe(1)
  })

  it('claims palette drops inside the container bounding box in relative coordinates', () => {
    const nodes = [loop, body, free]
    expect(findDropContainer(nodes, { x: 700, y: 200 })).toBe('loop-1')
    expect(findDropContainer(nodes, { x: 620, y: 440 })).toBeUndefined()
    // Loop manifests never nest, even when dropped inside a container.
    expect(findDropContainer([loop, { ...loop, id: 'loop-2', position: { x: 700, y: 200 } }, body], { x: 700, y: 200 })).toBe('loop-1')
  })

  it('reports how far a dragged child escapes its container', () => {
    const container = { ...loop, width: 470, height: 300 }
    expect(containerEscapeOverflow({ position: { x: 40, y: 130 }, width: 240, height: 120 }, container)).toBe(0)
    expect(containerEscapeOverflow({ position: { x: -70, y: 0 }, width: 240, height: 120 }, container)).toBe(70)
    expect(containerEscapeOverflow({ position: { x: 400, y: 0 }, width: 240, height: 120 }, container)).toBe(120)
  })

  it('derives view-only boundary links around the real body graph', () => {
    const edges: StudioEdge[] = [{ id: 'inside', source: 'body-1', sourceHandle: 'main', target: 'body-2', targetHandle: 'main', data: { edgeKind: 'execution', order: 0 } }]
    const containers = indexContainers([loop, body, { ...body, id: 'body-2', position: { x: 320, y: 130 }, data: { ...body.data, key: 'body_2' } }])
    const links = loopBoundaryEdges(containers, edges, new Map([['set@1', studioManifest('set')]]))

    expect(links.map(link => [link.source, link.sourceHandle, link.target, link.targetHandle])).toEqual([['loop-1::iteration-start', 'main', 'body-1', 'main'], ['body-2', 'main', 'loop-1::iteration-end', 'main'], ['body-2', 'error', 'loop-1::iteration-end', 'error']])
    expect(links.every(link => link.data?.derived && link.selectable === false)).toBe(true)
  })
})
