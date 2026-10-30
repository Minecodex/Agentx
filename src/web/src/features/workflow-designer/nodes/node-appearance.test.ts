import { describe, expect, it } from 'vitest'

import type { NodeManifest } from '../model/types'
import { canvasNodeMetrics, findOpenCanvasPosition, groupColor, nodeGroup, nodeShape } from './node-appearance'

const manifest = (overrides: Partial<NodeManifest>): NodeManifest => ({
  protocolVersion: '2.0', nodeType: 'node', version: 1, displayName: 'Node', description: '', category: 'actions', keywords: [], iconKey: 'box', executionStyle: 'action', capability: 'builtin', readiness: 'any', inputPorts: [], outputPorts: [], bindingSlots: [], parameterSchema: {}, uiSchema: {}, providers: [], credentials: [], retryPolicy: { retryable: false, maxAttempts: 1, initialBackoffMs: 0, maxBackoffMs: 0 }, sandboxRequired: false, supportsMock: true, sideEffectLevel: 'none', ...overrides,
})

describe('workflow node appearance', () => {
  it('maps node types onto the six visual groups with semantic theme tokens', () => {
    expect(nodeGroup('start')).toBe('start')
    expect(nodeGroup('boundary')).toBe('start')
    expect(nodeGroup('agent')).toBe('ai')
    expect(nodeGroup('model')).toBe('ai')
    expect(nodeGroup('if')).toBe('logic')
    expect(nodeGroup('merge')).toBe('logic')
    expect(nodeGroup('loop_over_items')).toBe('logic')
    expect(nodeGroup('approval')).toBe('logic')
    expect(nodeGroup('code')).toBe('transform')
    expect(nodeGroup('set')).toBe('transform')
    expect(nodeGroup('list')).toBe('transform')
    expect(nodeGroup('declarative_http')).toBe('integrate')
    expect(nodeGroup('sub_workflow')).toBe('integrate')
    expect(nodeGroup('exit')).toBe('output')
    expect(nodeGroup('brand_new_node')).toBe('integrate')
    expect(groupColor('start')).toBe('var(--studio-group-start)')
    expect(groupColor('ai')).toBe('var(--studio-group-ai)')
    expect(groupColor('logic')).toBe('var(--studio-group-logic)')
    expect(groupColor('transform')).toBe('var(--studio-group-transform)')
    expect(groupColor('integrate')).toBe('var(--studio-group-integrate)')
    expect(groupColor('output')).toBe('var(--studio-group-output)')
  })

  it('falls back to default when an old Manifest does not declare a role', () => {
    expect(nodeShape(manifest({ executionStyle: 'trigger' }))).toBe('default')
    expect(nodeShape(manifest({ nodeType: 'if', category: 'flow' }))).toBe('default')
    expect(nodeShape(manifest({ capability: 'agent' }))).toBe('default')
    expect(nodeShape(manifest({ capability: 'sandbox' }))).toBe('default')
  })

  it('uses content-sized cards and the declared role', () => {
    expect(nodeShape(manifest({ uiSchema: { canvas: { role: 'code' } } }))).toBe('code')
    for (const role of ['default', 'flow', 'trigger', 'merge', 'suspend', 'sub_workflow', 'code'] as const) expect(canvasNodeMetrics(role)).toMatchObject({ width: 240, height: 104 })
    for (const role of ['agent', 'branch', 'approval'] as const) expect(canvasNodeMetrics(role)).toMatchObject({ width: 280, height: 104 })
    expect(canvasNodeMetrics('agent', { bodyRows: 2, attachmentRows: 2 })).toMatchObject({ width: 280, height: 168 })
    expect(canvasNodeMetrics('default', { bodyRows: 8 })).toMatchObject({ width: 240, height: 144 })
  })

  it('uses 32px rows and rounds card heights up to 8px', () => {
    expect(canvasNodeMetrics('branch', { bodyRows: 0, branchRows: 3 })).toMatchObject({ width: 280, height: 176 })
    expect(canvasNodeMetrics('approval', { bodyRows: 0, branchRows: 2 })).toMatchObject({ width: 280, height: 144 })
    expect(canvasNodeMetrics('merge', { bodyRows: 1, inputRows: 3 })).toMatchObject({ width: 280, height: 200 })
    expect(canvasNodeMetrics('merge', { bodyRows: 1, inputRows: 3, attachmentRows: 1 })).toMatchObject({ width: 280, height: 224 })
  })

  it('centers a clicked node and moves it to the nearest open position when occupied', () => {
    expect(findOpenCanvasPosition({ x: 500, y: 400 }, { width: 96, height: 96 }, [])).toEqual({ x: 452, y: 352 })
    expect(findOpenCanvasPosition(
      { x: 500, y: 400 },
      { width: 96, height: 96 },
      [{ x: 440, y: 340, width: 112, height: 96 }],
    )).toEqual({ x: 620, y: 352 })
    expect(findOpenCanvasPosition(
      { x: 500, y: 500 },
      { width: 240, height: 160 },
      [{ x: 380, y: 420, width: 240, height: 160 }],
      { x: 0, y: 0, width: 1000, height: 760 },
    )).toEqual({ x: 692, y: 420 })
  })
})
