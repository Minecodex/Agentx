import { describe, expect, it } from 'vitest'
import { routeFromPoints, routeIsClear, routeToolbarPoint, temporaryRoute } from './canvas-route'
import { routeWaypoints, routingWorld } from './routing-world'
import type { CanvasNode, StudioEdge } from '../model/types'

const node = (id: string, x: number, y: number, parentId?: string): CanvasNode => ({ id, type: 'manifest', parentId, position: { x, y }, width: 240, height: 104, data: { editorKind: 'action', nodeType: 'set', typeVersion: 1, label: id, key: id, parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false } })
const edge = (id: string, source: string, target: string, error = false): StudioEdge => ({ id, source, target, sourceHandle: error ? 'error' : 'main', data: { edgeKind: 'execution', sourcePortKind: error ? 'error' : 'main' } })

describe('canvas route geometry', () => {
  it('places toolbars on usable segments and hides them when zoom leaves no room', () => {
    const points = [{ x: 240, y: 24 }, { x: 336, y: 24 }]
    const obstacles = [{ x: 0, y: 0, width: 240, height: 104 }, { x: 336, y: 0, width: 240, height: 104 }, { x: 252, y: 12, width: 24, height: 24 }]
    expect(routeToolbarPoint(points, obstacles, 1)).toEqual({ x: 306, y: 24 })
    expect(routeToolbarPoint(points, obstacles, .5)).toBeUndefined()
  })
  it('draws rounded orthogonal corners and places the toolbar on an actual segment', () => {
    const route = routeFromPoints([{ x: 0, y: 24 }, { x: 24, y: 24 }, { x: 48, y: 24 }, { x: 48, y: 124 }, { x: 240, y: 124 }])
    expect(route.points).toHaveLength(4)
    expect(route.path).toContain('Q 48 24 48 36')
    expect(route.path).not.toContain('C')
    expect(route.label).toEqual({ x: 144, y: 124 })
    expect(route.bounds).toEqual({ x: 0, y: 24, width: 240, height: 100 })
  })

  it('rejects paths through nodes, diagonal library failures and paths outside a loop', () => {
    const obstacles = [{ x: 0, y: 0, width: 240, height: 104 }, { x: 400, y: 0, width: 240, height: 104 }]
    const path = [{ x: 240, y: 24 }, { x: 400, y: 24 }]
    expect(routeIsClear(path, obstacles)).toBe(true)
    expect(routeIsClear(path, [...obstacles, { x: 300, y: 0, width: 40, height: 104 }])).toBe(false)
    expect(routeIsClear([{ x: 0, y: 0 }, { x: 100, y: 50 }], [])).toBe(false)
    expect(routeIsClear(path, [], { x: 0, y: 0, width: 300, height: 280 })).toBe(false)
    expect(temporaryRoute(path[0], path[1]).status).toBe('pending')
  })

  it('includes third-party obstacles and gives error and reverse edges separate lower lanes', () => {
    const nodes = [node('a', 0, 0), node('b', 500, 0), node('obstacle', 280, 240)]
    const edges = [edge('normal', 'a', 'b'), edge('error', 'a', 'b', true), edge('reverse', 'b', 'a')]
    const domain = routingWorld(nodes, edges).domainFor('a', 'b')
    expect(domain.obstacles.map(box => box.id)).toContain('obstacle')
    expect(routeWaypoints(domain, 'normal', { x: 240, y: 24 }, { x: 500, y: 24 }, false)).toEqual([])
    const error = routeWaypoints(domain, 'error', { x: 240, y: 52 }, { x: 500, y: 24 }, true)
    const reverse = routeWaypoints(domain, 'reverse', { x: 740, y: 24 }, { x: 0, y: 24 }, false)
    expect(error[1].y).toBeGreaterThan(344)
    expect(reverse[1].y - error[1].y).toBeGreaterThanOrEqual(12)
    const moved = routingWorld(nodes.map(n => n.id === 'obstacle' ? { ...n, position: { x: 280, y: 0 } } : n), edges).domainFor('a', 'b')
    expect(moved.obstacles.find(box => box.id === 'obstacle')?.y).toBe(0)
  })

  it('converts loop-relative positions once and excludes expanded group frames', () => {
    const loop = { ...node('loop', 700, 300), width: 1000, height: 500 }
    const a = node('a', 304, 84, 'loop'), b = node('b', 640, 84, 'loop')
    const group: CanvasNode = { id: 'g', type: 'group', parentId: 'loop', position: { x: 270, y: 48 }, width: 650, height: 240, data: { editorKind: 'group', groupId: 'g', label: 'Group', collapsed: false, memberCount: 2, onToggle() {}, onRemove() {} } }
    const world = routingWorld([loop, a, b, group], [edge('inner', 'a', 'b')])
    const domain = world.domainFor('a', 'b')
    expect(domain.frame).toMatchObject({ x: 700, y: 300, width: 1000, height: 500 })
    expect(domain.obstacles).toHaveLength(3)
    expect(domain.obstacles[0]).toMatchObject({ x: 1004, y: 384 })
    expect(domain.avoidAreas).toHaveLength(5)
    expect(world.domainFor('loop', 'outside').obstacles.map(box => box.id)).toEqual(['loop'])
  })
})
