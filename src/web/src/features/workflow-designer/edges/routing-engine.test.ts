import { Position } from '@xyflow/react'
import { routeSmartEdgeBatch, type SmartEdgeBatchItem } from '@tisoap/react-flow-smart-edge'
import { describe, expect, it } from 'vitest'
import type { CanvasNode, StudioEdge } from '../model/types'
import { routingWorld, routeWaypoints } from './routing-world'
import { routeIsClear, orthogonalRoutePoints } from './canvas-route'

const node = (id: string, x: number, y: number, width = 240, height = 104, parentId?: string): CanvasNode => ({ id, type: 'manifest', parentId, position: { x, y }, width, height, measured: { width, height }, data: { editorKind: 'action', nodeType: 'set', typeVersion: 1, label: id, key: id, parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false } })
const exit = (id: string, x: number, y: number): CanvasNode => ({ id, type: 'exit', position: { x, y }, width: 184, height: 80, data: { editorKind: 'exit', key: id, label: id, protected: false, parameters: { outputs: {}, errorOutputs: {} } } })

function route(nodes: CanvasNode[], edges: StudioEdge[], edge: SmartEdgeBatchItem) {
  const domain = routingWorld(nodes, edges).domainFor(edge.source, edge.target)
  const source = { x: edge.sourceX, y: edge.sourceY }, target = { x: edge.targetX, y: edge.targetY }
  const connection = edges.find(value => value.id === edge.id)
  const error = connection?.sourceHandle === 'error' || connection?.data?.sourcePortKind === 'error'
  const result = routeSmartEdgeBatch(nodes, [{ ...edge, options: { nodePadding: 16, gridRatio: 8, borderRadius: 12, avoidAreas: domain.avoidAreas }, waypoints: routeWaypoints(domain, edge.id, source, target, error) }])[edge.id]
  expect(result?.kind).toBe('routed')
  if (result?.kind !== 'routed') throw new Error('No route')
  const points = orthogonalRoutePoints([source, ...result.points.map(([x, y]) => ({ x, y })), target])
  expect(routeIsClear(points, domain.obstacles, domain.frame), JSON.stringify(points)).toBe(true)
  return points
}
const edge: SmartEdgeBatchItem = { id: 'edge', source: 'a', target: 'b', sourceX: 240, sourceY: 24, targetX: 640, targetY: 24, sourcePosition: Position.Right, targetPosition: Position.Left, preset: 'smoothstep' }

describe('installed smart routing engine', () => {
  it('reroutes a straight connection when an unconnected third node obstructs it', () => {
    const nodes = [node('a', 0, 0), node('b', 640, 0), node('third', 350, 220, 120)]
    const edges: StudioEdge[] = [{ id: edge.id, source: 'a', target: 'b', sourceHandle: 'main' }]
    route(nodes, edges, edge)
    const changed = route(nodes.map(n => n.id === 'third' ? node('third', 350, 0, 120) : n), edges, edge)
    expect(changed.some(point => point.y < 0 || point.y > 104)).toBe(true)
  })

  it('uses lower lanes for error and reverse connections', () => {
    const nodes = [node('a', 0, 0), node('b', 640, 0)]
    route(nodes, [{ id: edge.id, source: 'a', target: 'b', sourceHandle: 'error' }], { ...edge, sourceY: 52 })
    route(nodes, [{ id: edge.id, source: 'b', target: 'a', sourceHandle: 'main' }], { ...edge, source: 'b', target: 'a', sourceX: 880, targetX: 0 })
  })

  it('keeps each branch main/error pair local to its own End despite lower unrelated cards', () => {
    const nodes = [node('unrelated', 350, 1200)]
    const edges: StudioEdge[] = []
    for (const [index, id] of ['a', 'b', 'c'].entries()) {
      nodes.push(node(id, 0, index * 240), exit(`end-${id}`, 640, index * 240))
      edges.push({ id: `main-${id}`, source: id, sourceHandle: 'main', target: `end-${id}`, targetHandle: 'main' }, { id: `error-${id}`, source: id, sourceHandle: 'error', target: `end-${id}`, targetHandle: 'error' })
    }
    for (const [index, id] of ['a', 'b', 'c'].entries()) {
      const points = route(nodes, edges, { ...edge, id: `error-${id}`, source: id, target: `end-${id}`, sourceY: index * 240 + 52, targetY: index * 240 + 52 })
      expect(points.every(point => point.y === index * 240 + 52)).toBe(true)
    }
  })

  it('routes to a dedicated error End locally and still avoids a moved third-party obstacle', () => {
    const nodes = [node('a', 0, 100), exit('error-end', 640, 100), node('third', 350, 900, 120)]
    const edges: StudioEdge[] = [{ id: edge.id, source: 'a', sourceHandle: 'error', target: 'error-end', targetHandle: 'error' }]
    const request = { ...edge, target: 'error-end', sourceY: 152, targetY: 152 }
    expect(route(nodes, edges, request).every(point => point.y === 152)).toBe(true)
    const points = route(nodes.map(value => value.id === 'third' ? node('third', 350, 100, 120) : value), edges, request)
    expect(points.some(point => point.y !== 152)).toBe(true)
    expect(Math.max(...points.map(point => point.y))).toBeLessThan(500)
  })

  it('keeps distinct condition branches clear when both destinations are below the source', () => {
    const a = node('a', 0, 0, 280, 176), b = node('b', 640, 240), c = node('c', 640, 480)
    const edges = [{ id: 'first', source: 'a', sourceHandle: 'case:first', target: 'b' }, { id: 'second', source: 'a', sourceHandle: 'case:second', target: 'c' }]
    const first = route([a, b, c], edges, { ...edge, id: 'first', sourceX: 280, sourceY: 84, targetY: 264 })
    const second = route([a, b, c], edges, { ...edge, id: 'second', target: 'c', sourceX: 280, sourceY: 116, targetY: 504 })
    const vertical = (points: typeof first) => points.slice(1).flatMap((end, index) => points[index].x === end.x ? [{ x: end.x, top: Math.min(points[index].y, end.y), bottom: Math.max(points[index].y, end.y) }] : [])
    const shared = vertical(first).some(a => vertical(second).some(b => Math.abs(a.x - b.x) < 12 && Math.min(a.bottom, b.bottom) > Math.max(a.top, b.top)))
    expect(shared, JSON.stringify({ first, second })).toBe(false)
  })

  it('keeps multiple merge inputs in separate approach lanes', () => {
    const nodes = [node('a', 0, 0), node('b', 0, 240), node('merge', 640, 320, 280, 152)]
    const edges = [{ id: 'first', source: 'a', target: 'merge', targetHandle: 'left' }, { id: 'second', source: 'b', target: 'merge', targetHandle: 'right' }]
    const first = route(nodes, edges, { ...edge, id: 'first', target: 'merge', targetY: 404 })
    const second = route(nodes, edges, { ...edge, id: 'second', source: 'b', target: 'merge', sourceY: 264, targetY: 436 })
    const guideA = routingWorld(nodes, edges).domainFor('a', 'merge').channels.get('first')
    expect(guideA).toEqual({ side: 'target', index: 0 })
    expect(Math.max(...first.map(point => point.y))).toBeGreaterThanOrEqual(404)
    expect(Math.max(...second.map(point => point.y))).toBeGreaterThanOrEqual(436)
  })

  it('routes collapsed proxy error ports without crossing the proxy', () => {
    const proxy: CanvasNode = { id: 'group:g', type: 'group', position: { x: 640, y: 200 }, width: 240, height: 104, data: { editorKind: 'group', groupId: 'g', collapsed: true, label: 'Group', memberCount: 2, onToggle() {}, onRemove() {} } }
    const nodes = [node('a', 0, 0), proxy, node('b', 1280, 200)]
    const edges: StudioEdge[] = [{ id: 'first', source: 'a', sourceHandle: 'error', target: proxy.id, targetHandle: 'group-error-in' }, { id: 'second', source: proxy.id, sourceHandle: 'group-error-out', target: 'b', targetHandle: 'error', data: { edgeKind: 'execution', sourcePortKind: 'error' } }]
    route(nodes, edges, { ...edge, id: 'first', target: proxy.id, sourceY: 52, targetY: 252 })
    route(nodes, edges, { ...edge, id: 'second', source: proxy.id, sourceX: 880, sourceY: 252, targetX: 1280, targetY: 252 })
  })

  it('keeps error convergence from distinct sources in separate lower lanes', () => {
    const nodes = [node('a', 0, 0), node('b', 0, 240), node('end', 960, 0, 184, 80)]
    const edges = [{ id: 'first', source: 'a', sourceHandle: 'error', target: 'end', targetHandle: 'error' }, { id: 'second', source: 'b', sourceHandle: 'error', target: 'end', targetHandle: 'error' }]
    const first = route(nodes, edges, { ...edge, id: 'first', target: 'end', sourceY: 52, targetX: 960, targetY: 52 })
    const second = route(nodes, edges, { ...edge, id: 'second', source: 'b', target: 'end', sourceY: 292, targetX: 960, targetY: 52 })
    expect(Math.max(...second.map(point => point.y)) - Math.max(...first.map(point => point.y))).toBeGreaterThanOrEqual(12)
  })

  it('routes internal edges inside their loop using absolute coordinates', () => {
    const nodes = [node('loop', 700, 300, 1280, 500), node('a', 304, 84, 240, 104, 'loop'), node('b', 800, 84, 240, 104, 'loop'), node('third', 620, 84, 100, 120, 'loop')]
    route(nodes, [{ id: edge.id, source: 'a', target: 'b', sourceHandle: 'main' }], { ...edge, sourceX: 1244, sourceY: 408, targetX: 1500, targetY: 408 })
  })
})
