import { ReactFlowProvider, type NodeProps } from '@xyflow/react'
import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { CanvasNode } from '../model/types'
import { createEditorStore, EditorStoreProvider } from '../store/editor-store'
const useEditorStore = createEditorStore()
import { AnnotationNode, GroupNode, IterationChipNode, IterationEndNode, LoopContainerNode } from './editor-overlays'

describe('editor overlay nodes', () => {
  it('edits and colors a Sticky Note without entering the Workflow Definition', () => {
    const onChange = vi.fn()
    const props = { id: 'annotation:note', selected: true, data: { editorKind: 'annotation', annotationId: 'note', text: 'Initial note', color: 'yellow', onChange, onRemove: vi.fn(), onResizeStart: vi.fn(), onResize: vi.fn(), onResizeEnd: vi.fn() } } as unknown as NodeProps<CanvasNode>
    render(<EditorStoreProvider store={useEditorStore}><ReactFlowProvider><AnnotationNode {...props} /></ReactFlowProvider></EditorStoreProvider>)

    fireEvent.doubleClick(screen.getByTestId('studio-note-note'))
    const editor = screen.getByRole('textbox')
    fireEvent.change(editor, { target: { value: 'Updated note' } })
    fireEvent.blur(editor)
    expect(onChange).toHaveBeenCalledWith({ text: 'Updated note' })
    fireEvent.click(screen.getByRole('button', { name: /Note color: blue|便签颜色：blue/ }))
    expect(onChange).toHaveBeenCalledWith({ color: 'blue' })
  })

  it('renders a collapsed Group proxy with aggregate handles and ungroup action', () => {
    const onToggle = vi.fn()
    const onRemove = vi.fn()
    const props = { id: 'group:main', selected: false, data: { editorKind: 'group', groupId: 'main', label: 'Main', collapsed: true, memberCount: 3, onToggle, onRemove } } as unknown as NodeProps<CanvasNode>
    render(<EditorStoreProvider store={useEditorStore}><ReactFlowProvider><GroupNode {...props} /></ReactFlowProvider></EditorStoreProvider>)

    const group = screen.getByTestId('studio-group-main')
    expect(group.querySelector('[data-nodeid]')).not.toBeInTheDocument()
    expect(group.querySelectorAll('.react-flow__handle')).toHaveLength(2)
    fireEvent.click(screen.getByRole('button', { name: /Expand group|展开分组/ }))
    fireEvent.click(screen.getByRole('button', { name: /Ungroup|解除分组/ }))
    expect(onToggle).toHaveBeenCalledOnce()
    expect(onRemove).toHaveBeenCalledOnce()
  })
})

describe('loop container overlays', () => {
  it('renders the loop container frame with header, parallel chip, footer note and ports', () => {
    const props = { id: 'loop-1', selected: false, data: { editorKind: 'loop-container', loopId: 'loop-1', label: '循环处理', parallelism: 6, childCount: 0, boundaryLinks: [] } } as unknown as NodeProps<CanvasNode>
    render(<EditorStoreProvider store={useEditorStore}><ReactFlowProvider><LoopContainerNode {...props} /></ReactFlowProvider></EditorStoreProvider>)

    const frame = screen.getByTestId('studio-loop-loop-1')
    expect(frame.className).toContain('studio-loop-container')
    expect(screen.getByTestId('studio-loop-icon-loop-1')).toBeInTheDocument()
    expect(screen.getByText('循环处理')).toBeInTheDocument()
    expect(screen.getByTestId('studio-loop-parallel-loop-1').textContent).toMatch(/并行 ×6|Parallel ×6/)
    expect(frame.textContent).toMatch(/内置变量|Built-in variables/)
    expect(frame.textContent).toMatch(/输入|Input/)
    expect(frame.textContent).toMatch(/输出|Output/)
    expect(frame.textContent).toMatch(/错误|Error/)
    const handles = frame.querySelectorAll('.react-flow__handle')
    expect(handles).toHaveLength(3)
    expect([...handles].map((handle) => handle.getAttribute('data-handlepos'))).toEqual(['left', 'right', 'right'])
  })

  it('resizes the canonical loop from a corner pointer gesture', () => {
    useEditorStore.setState({ nodes: [{ id: 'loop-1', type: 'manifest', position: { x: 20, y: 30 }, width: 470, height: 300, data: { editorKind: 'action', nodeType: 'loop_over_items', typeVersion: 1, label: '循环处理', key: 'loop', parameters: {}, contextWrites: [], resourceReferences: [], settings: {}, disabled: false } }], past: [], future: [], dirty: false })
    const props = { id: 'loop-1', selected: true, data: { editorKind: 'loop-container', loopId: 'loop-1', label: '循环处理', parallelism: 1, childCount: 0, boundaryLinks: [] } } as unknown as NodeProps<CanvasNode>
    render(<EditorStoreProvider store={useEditorStore}><ReactFlowProvider><LoopContainerNode {...props} /></ReactFlowProvider></EditorStoreProvider>)

    const corner = screen.getByRole('button', { name: /bottom-right/ })
    fireEvent.pointerDown(corner, { pointerId: 1, clientX: 100, clientY: 100 })
    fireEvent.pointerMove(corner, { pointerId: 1, clientX: 220, clientY: 180 })
    fireEvent.pointerUp(corner, { pointerId: 1, clientX: 220, clientY: 180 })

    expect(useEditorStore.getState().nodes[0]).toMatchObject({ position: { x: 20, y: 30 }, width: 590, height: 380 })
    expect(useEditorStore.getState().past).toHaveLength(1)
  })

  it('renders the iteration start chip with a single source handle', () => {
    const props = { id: 'loop-1::iteration-start', selected: false, data: { editorKind: 'iteration-chip', loopId: 'loop-1' } } as unknown as NodeProps<CanvasNode>
    render(<EditorStoreProvider store={useEditorStore}><ReactFlowProvider><IterationChipNode {...props} /></ReactFlowProvider></EditorStoreProvider>)

    const chip = screen.getByTestId('studio-chip-loop-1')
    expect(chip.className).toContain('studio-loop-chip')
    expect(chip.textContent).toMatch(/迭代开始|Iteration start/)
    const handles = chip.querySelectorAll('.react-flow__handle')
    expect(handles).toHaveLength(1)
    expect(handles[0].getAttribute('data-handleid')).toBe('main')
    expect(handles[0].getAttribute('data-handlepos')).toBe('right')
  })

  it('renders the iteration end chip with success and error target handles', () => {
    const props = { id: 'loop-1::iteration-end', selected: false, data: { editorKind: 'iteration-end', loopId: 'loop-1' } } as unknown as NodeProps<CanvasNode>
    render(<EditorStoreProvider store={useEditorStore}><ReactFlowProvider><IterationEndNode {...props} /></ReactFlowProvider></EditorStoreProvider>)

    const chip = screen.getByTestId('studio-end-chip-loop-1')
    expect(chip.textContent).toMatch(/迭代结束|Iteration end/)
    const handles = chip.querySelectorAll('.react-flow__handle')
    expect(handles).toHaveLength(2)
    expect([...handles].map((handle) => handle.getAttribute('data-handleid'))).toEqual(['main', 'error'])
    expect([...handles].map((handle) => handle.getAttribute('data-handlepos'))).toEqual(['left', 'left'])
  })
})
