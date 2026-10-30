import { describe, expect, it } from 'vitest'
import { studioManifest, studioManifestTypes } from '../testing/studio-catalog'
import type { ActionNodeData } from '../model/types'
import { actionNodeGeometry, boundaryGeometry, studioNodeGeometry } from './canvas-node-geometry'
import { createEditorStore } from '../store/editor-store'

const data = (nodeType: string, parameters: Record<string, unknown> = {}): ActionNodeData => ({ editorKind: 'action', nodeType, typeVersion: 1, label: 'A long title '.repeat(20), key: nodeType, parameters, contextWrites: [], resourceReferences: [], settings: {}, disabled: false })

describe('shared card geometry', () => {
  for (const nodeType of studioManifestTypes) it(`has fixed main/error baselines for ${nodeType}`, () => {
    const manifest = studioManifest(nodeType)
    const geometry = actionNodeGeometry(data(nodeType), manifest)
    expect(geometry.height % 8).toBe(0)
    expect(geometry.height).toBeGreaterThanOrEqual(104)
    for (const port of geometry.ports) {
      expect(port.x).toBe(port.direction === 'input' ? 0 : geometry.width)
      expect(port.y).toBe(port.kind === 'error' ? 52 : port.row === undefined ? 24 : 68 + port.row * 32 + 16)
      expect(port.y).toBeLessThan(geometry.height)
    }
    expect(geometry.ports.some(port => port.id === 'case' || port.id === 'decision')).toBe(false)
  })

  it('aligns dynamic branches with row centers and keeps errors out of the list', () => {
    const geometry = actionNodeGeometry(data('if', { cases: [{ id: 'first' }, { id: 'second' }] }), studioManifest('if'))
    expect(geometry.width).toBe(280)
    expect(geometry.rowCount).toBe(3)
    expect(geometry.ports.filter(port => port.direction === 'output').map(port => [port.id, port.y])).toEqual([['case:first', 84], ['case:second', 116], ['else', 148], ['error', 52]])
    expect(geometry.height).toBe(176)
  })

  it('uses 24px for a single merge input and 32px rows for combine inputs', () => {
    const manifest = studioManifest('merge')
    const single = actionNodeGeometry(data('merge', { mode: 'append' }), manifest)
    expect(single.ports.filter(port => port.direction === 'input').map(port => [port.id, port.y])).toEqual([['main', 24]])
    const multiple = actionNodeGeometry(data('merge', { mode: 'combine_by_key' }), manifest)
    expect(multiple.width).toBe(280)
    expect(multiple.ports.filter(port => port.direction === 'input').map(port => [port.id, port.y])).toEqual([['left', 84], ['right', 116]])
  })

  it('does not shift fixed ports when summaries or attachments grow', () => {
    const manifest = studioManifest('agent')
    const empty = data('agent')
    const rich: ActionNodeData = { ...empty, parameters: { maxIterations: 12 }, resourceReferences: (['model', 'mcp_tool', 'rag', 'skill', 'memory'] as const).map(resourceType => ({ bindingRole: resourceType, resourceType, resourceId: resourceType, resourceVersionId: 'v1', operation: 'use' })) }
    const before = actionNodeGeometry(empty, manifest), after = actionNodeGeometry(rich, manifest)
    expect(after.height).toBeGreaterThan(before.height)
    expect(after.ports).toEqual(before.ports)
    expect(after.width).toBe(280)
    expect(after.attachmentTop).toBe(after.summaryTop + 40)
  })

  it('sizes single and multi-port plugins through the same geometry', () => {
    const manifest = { ...studioManifest('set'), nodeType: 'acme.custom' }
    expect(actionNodeGeometry(data(manifest.nodeType), manifest).width).toBe(240)
    manifest.outputPorts = [...manifest.outputPorts, { name: 'extra', kind: 'main', required: false, variadic: false }]
    const multiple = actionNodeGeometry(data(manifest.nodeType), manifest)
    expect(multiple.width).toBe(280)
    expect(multiple.ports.find(port => port.id === 'extra')?.y).toBe(116)
  })

  it('shares 184×80 boundaries and clamps loop frames to their minimum', () => {
    expect(boundaryGeometry('start')).toMatchObject({ width: 184, height: 80 })
    expect(boundaryGeometry('exit').ports.map(port => port.y)).toEqual([24, 52])
    expect(studioNodeGeometry({ data: data('loop_over_items') })).toMatchObject({ width: 640, height: 360 })
    const loop = studioNodeGeometry({ data: data('loop_over_items'), width: 100, height: 100 })
    expect(loop).toMatchObject({ width: 520, height: 280 })
    expect(loop.ports.find(port => port.direction === 'output')?.x).toBe(520)
  })

  it('copies only card dimensions into an edit, keeping port geometry out of history', () => {
    const store = createEditorStore()
    const value = data('if', { cases: [{ id: 'yes' }, { id: 'no' }] })
    const geometry = actionNodeGeometry(value, studioManifest('if'))
    const id = store.getState().addAction(value, { x: 300, y: 200 }, geometry)
    expect(store.getState().nodes.find(node => node.id === id)).toMatchObject({ width: 280, height: 176 })
    expect(JSON.stringify(store.getState().past)).not.toMatch(/contentTop|rowCount|portName|CanvasRoute|avoidAreas/)
  })
})
