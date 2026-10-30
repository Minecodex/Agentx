import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { EdgeProps } from '@xyflow/react'
import type { StudioEdge } from '../model/types'
import { StudioEdgeComponent } from './studio-edge'

const fixture = vi.hoisted(() => ({
  root: undefined as HTMLElement | undefined,
  route: null as null | { kind: string; points?: number[][] },
  dragging: false,
  batches: 1,
  remove: vi.fn(),
  insert: vi.fn(),
}))
vi.mock('@tisoap/react-flow-smart-edge', () => ({ useSmartEdgePath: () => ({ route: fixture.route, isDragging: fixture.dragging }) }))
vi.mock('@xyflow/react', () => ({ BaseEdge: () => null, useStore: (select: (state: { domNode?: HTMLElement }) => unknown) => select({ domNode: fixture.root }), useViewport: () => ({ x: 0, y: 0, zoom: 1 }) }))
vi.mock('../store/editor-store', () => ({ useEditorStore: (select: (state: object) => unknown) => select({ readOnly: false, removeEdge: fixture.remove, requestEdgeInsert: fixture.insert }) }))
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }))
vi.mock('./routing-context', () => ({ useRoutingWorld: () => ({
  batches: fixture.batches, complete: true, dragging: fixture.dragging, hasQuickAdd: () => false,
  domainFor: () => ({ obstacles: [], bounds: { x: 0, y: 0, width: 1000, height: 400 }, avoidAreas: [], lanes: new Map(), channels: new Map() }),
}) }))

const props = { id: 'edge', source: 'a', target: 'b', sourceX: 240, sourceY: 24, targetX: 640, targetY: 24, sourcePosition: 'right', targetPosition: 'left', data: { edgeKind: 'execution' } } as EdgeProps<StudioEdge>
const state = (container: HTMLElement) => container.querySelector('[data-route-id]')!.getAttribute('data-route-state')

beforeEach(() => {
  fixture.route = null; fixture.dragging = false; fixture.batches = 1
  fixture.remove.mockClear(); fixture.insert.mockClear()
  fixture.root = document.createElement('div')
  fixture.root.innerHTML = '<div class="react-flow__renderer"></div>'
  document.body.append(fixture.root)
})
afterEach(() => { cleanup(); fixture.root?.remove() })

describe('route calculation states', () => {
  it('shows a pending preview until a real routed result arrives', () => {
    const view = render(<svg><StudioEdgeComponent {...props} /></svg>)
    expect(state(view.container)).toBe('pending')
    fixture.route = { kind: 'routed', points: [[240, 24], [640, 24]] }
    view.rerender(<svg><StudioEdgeComponent {...props} /></svg>)
    expect(state(view.container)).toBe('ready')
    expect(screen.queryByRole('status')).toBeNull()
  })

  it('reports library failure instead of accepting its native clear fallback', () => {
    fixture.route = { kind: 'clear' }
    const view = render(<svg><StudioEdgeComponent {...props} /></svg>)
    expect(state(view.container)).toBe('pending')
    fixture.batches += 1
    view.rerender(<svg><StudioEdgeComponent {...props} /></svg>)
    expect(state(view.container)).toBe('blocked')
    expect(screen.getByRole('status')).toHaveAttribute('title', 'studio.routing.blocked')
    expect(fixture.remove).not.toHaveBeenCalled()
  })

  it('keeps a drag preview temporary even when a previous route exists', () => {
    fixture.dragging = true
    fixture.route = { kind: 'routed', points: [[240, 24], [640, 24]] }
    const view = render(<svg><StudioEdgeComponent {...props} /></svg>)
    expect(state(view.container)).toBe('pending')
    expect(screen.queryByRole('status')).toBeNull()
  })
})
