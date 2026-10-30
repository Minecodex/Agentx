import { ReactFlowProvider, type NodeProps } from '@xyflow/react'
import { act, fireEvent, render, screen } from '@testing-library/react'
import { Profiler } from 'react'
import { describe, expect, it, vi } from 'vitest'

import type { CanvasNodeRole, NodeManifest, StudioNode } from '../model/types'
import { CanvasRenderProvider, createCanvasRenderStore } from '../store/canvas-render-store'
import { ManifestNode } from './manifest-node'
import * as localization from '../model/manifest-localization'

const canvasStore = createCanvasRenderStore()

const roles: CanvasNodeRole[] = ['default', 'trigger', 'branch', 'flow', 'merge', 'loop', 'suspend', 'approval', 'sub_workflow', 'agent', 'code']

const manifest = (role: CanvasNodeRole): NodeManifest => ({
  protocolVersion: '2.0', nodeType: role, version: role === 'agent' ? 2 : 1, displayName: role, description: `${role} node`, category: 'actions', keywords: [role], iconKey: 'box', executionStyle: 'action', capability: 'builtin', readiness: 'any',
  inputPorts: [{ name: 'main', kind: 'main', required: false, variadic: false }], outputPorts: [{ name: 'error', kind: 'error', required: false, variadic: false }], bindingSlots: role === 'agent' ? [{ name: 'model', resourceType: 'model', placement: 'inspector', required: true, multiple: false }, { name: 'workspace_sandbox', resourceType: 'sandbox_profile', placement: 'inspector', required: false, multiple: false }, { name: 'mcp_tools', resourceType: 'mcp_tool', placement: 'inspector', required: false, multiple: true }] : [],
  parameterSchema: {}, uiSchema: { canvas: { role } }, providers: [], credentials: [], retryPolicy: { retryable: false, maxAttempts: 1, initialBackoffMs: 0, maxBackoffMs: 0 }, sandboxRequired: false, supportsMock: true, sideEffectLevel: 'none',
})

const props = (role: CanvasNodeRole, data: Partial<Extract<StudioNode['data'], { editorKind: 'action' }>> = {}): NodeProps<StudioNode> => ({
  id: `node-${role}`, type: 'manifest', data: { editorKind: 'action', nodeType: role, typeVersion: role === 'agent' ? 2 : 1, label: `${role} label`, parameters: {}, resourceReferences: [], settings: {}, disabled: false, ...data }, selected: false,
} as unknown as NodeProps<StudioNode>)

describe('ManifestNode roles', () => {
  it('keeps card content stable through drag coordinates while refreshing data and runtime state', () => {
    const localize = vi.spyOn(localization, 'localizeManifest')
    canvasStore.setState({ manifests: new Map([['default@1', manifest('default')]]), runtimeStatuses: new Map(), bindingSummaries: new Map(), occupiedHandlesByNodeId: new Map(), zoomTier: 'full' })
    const value = props('default')
    const view = render(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...value} positionAbsoluteX={0} positionAbsoluteY={0} /></ReactFlowProvider></CanvasRenderProvider>)
    const before = localize.mock.calls.length
    view.rerender(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...value} positionAbsoluteX={100} positionAbsoluteY={180} dragging /></ReactFlowProvider></CanvasRenderProvider>)
    expect(localize).toHaveBeenCalledTimes(before)
    view.rerender(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...value} data={{ ...value.data, label: 'Updated card' }} positionAbsoluteX={100} positionAbsoluteY={180} /></ReactFlowProvider></CanvasRenderProvider>)
    expect(screen.getByText('Updated card')).toBeInTheDocument()
    act(() => canvasStore.setState({ runtimeStatuses: new Map([[value.id, 'failed']]) }))
    expect(screen.getByTestId(`studio-node-${value.id}`).querySelector('.studio-card')).toHaveClass('studio-node-failed')
    localize.mockRestore()
  })

  for (const role of roles) it(`renders the controlled ${role} role`, () => {
    const value = manifest(role)
    canvasStore.setState({ manifests: new Map([[`${role}@${value.version}`, value]]), runtimeStatuses: new Map(), bindingSummaries: new Map(), occupiedHandlesByNodeId: new Map(), zoomTier: 'full' })
    render(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...props(role)} /></ReactFlowProvider></CanvasRenderProvider>)
    const node = screen.getByTestId(`studio-node-node-${role}`)
    expect(node).toHaveAttribute('data-role', role)
    expect(node).toHaveStyle({ width: ['agent', 'branch', 'approval'].includes(role) ? '280px' : '240px' })
    expect(node.querySelector('.studio-card')).toBeInTheDocument()
    expect(node.querySelector('.studio-card-icon')).toBeInTheDocument()
    expect(node.querySelector('.studio-card-title')).toHaveTextContent(`${role} label`)
  })

  it('renders the card body summaries and attachment badges for an agent', () => {
    canvasStore.setState({
      manifests: new Map([['agent@2', manifest('agent')]]), runtimeStatuses: new Map([['node-agent', 'succeeded']]),
      bindingSummaries: new Map([['node-agent', [
        { role: 'mcp_tools', resourceType: 'mcp_tool', label: 'Weather tool' },
        { role: 'mcp_tools', resourceType: 'mcp_tool', label: 'Calendar tool' },
        { role: 'knowledge', resourceType: 'rag', label: 'SOP' },
      ]]]),
      occupiedHandlesByNodeId: new Map(),
      zoomTier: 'full',
    })
    render(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...props('agent', { parameters: { maxIterations: 8 }, resourceReferences: [{ bindingRole: 'model', resourceType: 'model', resourceId: 'gpt-4o', resourceVersionId: 'model-v1', operation: 'use' }] })} /></ReactFlowProvider></CanvasRenderProvider>)
    const node = screen.getByTestId('studio-node-node-agent')
    expect(node.querySelector('.studio-card')).toHaveClass('studio-node-succeeded')
    expect(node.querySelectorAll('.studio-card-row')).toHaveLength(2)
    expect(node.querySelectorAll('.studio-card-row')[0]).toHaveTextContent('gpt-4o')
    expect(node.querySelectorAll('.studio-card-row')[1]).toHaveTextContent(/最多 8 轮|Max 8 iterations/)
    expect(node.querySelector('.studio-card-att')).toHaveTextContent(/MCP [Tt]ool ×2/)
    expect(node.querySelector('.studio-card-att')).toHaveTextContent(/知识库|Knowledge/)
    expect(node.querySelector('.studio-card-att-add')).toHaveTextContent(/\+ 附件|\+ Attachment/)
    expect(node).toHaveStyle({ height: '168px' })
  })

  it('summarizes model, http, code, set, list, merge, and sub-workflow configuration', () => {
    const cases: Array<{ nodeType: string; parameters: Record<string, unknown>; pattern: RegExp }> = [
      { nodeType: 'model', parameters: { responseMode: 'json_schema' }, pattern: /结构化 JSON|Structured JSON/ },
      { nodeType: 'declarative_http', parameters: { method: 'POST', url: { kind: 'template', segments: [{ kind: 'text', text: 'https://api.test/tickets' }] } }, pattern: /POST.*api\.test/ },
      { nodeType: 'code', parameters: { runner: 'python' }, pattern: /Python/ },
      { nodeType: 'set', parameters: { values: { kind: 'object', fields: { status: { kind: 'literal', value: 'ok' }, total: { kind: 'literal', value: 1 } } } }, pattern: /设置 2 个字段|Set 2 fields/ },
      { nodeType: 'list', parameters: { filter: { conditions: [{ condition: true }] }, sort: [{ selector: 'score' }], takeN: 3 }, pattern: /筛选 1 条.*排序 1 条|1 filter.*1 sort/ },
      { nodeType: 'merge', parameters: { mode: 'append' }, pattern: /追加|Append/ },
      { nodeType: 'sub_workflow', parameters: { workflowVersionId: '9b2f1c4a-1111-4222-8333-444455556666' }, pattern: /9b2f1c4a/ },
    ]
    for (const { nodeType, parameters, pattern } of cases) {
      const value = manifest('default')
      value.nodeType = value.displayName = nodeType
      canvasStore.setState({ manifests: new Map([[`${nodeType}@1`, value]]), runtimeStatuses: new Map(), bindingSummaries: new Map(), occupiedHandlesByNodeId: new Map(), zoomTier: 'full' })
      const { unmount } = render(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...props('default', { nodeType, parameters })} /></ReactFlowProvider></CanvasRenderProvider>)
      expect(screen.getByTestId('studio-node-node-default').querySelector('.studio-card-body')).toHaveTextContent(pattern)
      unmount()
    }
  })

  it('renders if branch rows with per-case handles and skips the generic case/else ports', () => {
    const value = manifest('branch')
    value.nodeType = value.displayName = 'if'
    value.outputPorts = [
      { name: 'case', kind: 'main', required: false, variadic: true },
      { name: 'else', kind: 'main', required: false, variadic: false },
      { name: 'error', kind: 'error', required: false, variadic: false },
    ]
    canvasStore.setState({ manifests: new Map([['if@1', value]]), runtimeStatuses: new Map(), bindingSummaries: new Map(), occupiedHandlesByNodeId: new Map(), zoomTier: 'full' })
    render(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...props('branch', {
      nodeType: 'if',
      parameters: { cases: [
        { id: 'c1', name: 'high', conditions: [{ condition: 'amount >= 1000', label: '金额 ≥ 1000' }], logicalOp: 'and' },
        { id: 'c2', name: 'mid', conditions: [{ condition: 'amount >= 500', label: '金额 ≥ 500' }, { condition: 'level = vip' }], logicalOp: 'or' },
      ] },
    })} /></ReactFlowProvider></CanvasRenderProvider>)
    const node = screen.getByTestId('studio-node-node-branch')
    const rows = node.querySelectorAll('.studio-branch-row')
    expect(rows).toHaveLength(3)
    expect(rows[0]).toHaveTextContent('IF')
    expect(rows[0]).toHaveTextContent('金额 ≥ 1000')
    expect(rows[1]).toHaveTextContent('ELIF')
    expect(rows[1]).toHaveTextContent('金额 ≥ 500')
    expect(rows[1]).toHaveTextContent(/条件 2|Case 2/)
    expect(rows[1].querySelector('.studio-branch-badge')).toHaveTextContent(/或|OR/)
    expect(rows[2]).toHaveTextContent('ELSE')
    expect(node.querySelector('.react-flow__handle.source[data-handleid="error"]')).toHaveStyle({ top: '52px' })
    expect(node.querySelector('.react-flow__handle.source[data-handleid="case:c1"]')).toBeInTheDocument()
    expect(node.querySelector('.react-flow__handle.source[data-handleid="case:c2"]')).toBeInTheDocument()
    expect(node.querySelectorAll('.react-flow__handle[data-handleid="else"]')).toHaveLength(1)
    expect(node.querySelector('.react-flow__handle.source[data-handleid="error"]')).toBeInTheDocument()
    expect(node.querySelector('.react-flow__handle[data-handleid="case"]')).not.toBeInTheDocument()
    expect(node.querySelector('[data-port-id="case"]')).not.toBeInTheDocument()
    expect(node).toHaveStyle({ height: '176px' })
  })

  it('falls back to the default approval buttons when none are configured', () => {
    const value = manifest('approval')
    value.nodeType = value.displayName = 'approval'
    value.outputPorts = [
      { name: 'decision', kind: 'main', required: false, variadic: true },
      { name: 'timed_out', kind: 'main', required: false, variadic: false },
      { name: 'error', kind: 'error', required: false, variadic: false },
    ]
    canvasStore.setState({ manifests: new Map([['approval@1', value]]), runtimeStatuses: new Map(), bindingSummaries: new Map(), occupiedHandlesByNodeId: new Map(), zoomTier: 'full' })
    render(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...props('approval', { nodeType: 'approval', parameters: {} })} /></ReactFlowProvider></CanvasRenderProvider>)
    const node = screen.getByTestId('studio-node-node-approval')
    const rows = node.querySelectorAll('.studio-branch-row')
    expect(rows).toHaveLength(3)
    expect(rows[0]).toHaveTextContent(/按钮|Button/)
    expect(rows[0]).toHaveTextContent(/通过|Approve/)
    expect(rows[1]).toHaveTextContent(/拒绝|Reject/)
    expect(rows[2]).toHaveTextContent(/超时|Timeout/)
    expect(node.querySelector('.react-flow__handle.source[data-handleid="error"]')).toHaveStyle({ top: '52px' })
    expect(node.querySelector('.react-flow__handle.source[data-handleid="decision:approved"]')).toBeInTheDocument()
    expect(node.querySelector('.react-flow__handle.source[data-handleid="decision:rejected"]')).toBeInTheDocument()
    expect(node.querySelector('.react-flow__handle.source[data-handleid="timed_out"]')).toBeInTheDocument()
    expect(node.querySelector('.react-flow__handle.source[data-handleid="error"]')).toBeInTheDocument()
    expect(node.querySelector('.react-flow__handle[data-handleid="decision"]')).not.toBeInTheDocument()
    expect(node).toHaveStyle({ height: '176px' })
  })

  it('renders configured approval buttons and the timeout row with hours', () => {
    const onQuickAdd = vi.fn()
    const value = manifest('approval')
    value.nodeType = value.displayName = 'approval'
    value.outputPorts = [
      { name: 'decision', kind: 'main', required: false, variadic: true },
      { name: 'timed_out', kind: 'main', required: false, variadic: false },
      { name: 'error', kind: 'error', required: false, variadic: false },
    ]
    canvasStore.setState({ manifests: new Map([['approval@1', value]]), runtimeStatuses: new Map(), bindingSummaries: new Map(), occupiedHandlesByNodeId: new Map(), zoomTier: 'full', onQuickAdd })
    render(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...props('approval', { nodeType: 'approval', parameters: { buttons: [{ id: 'ok', label: '同意' }], timeoutMs: 90_000_000 } })} /></ReactFlowProvider></CanvasRenderProvider>)
    const node = screen.getByTestId('studio-node-node-approval')
    const rows = node.querySelectorAll('.studio-branch-row')
    expect(rows).toHaveLength(2)
    expect(rows[0]).toHaveTextContent('同意')
    expect(rows[1]).toHaveTextContent(/25 小时|25 h/)
    expect(node.querySelector('.react-flow__handle.source[data-handleid="decision:ok"]')).toBeInTheDocument()
    expect(node.querySelector('.react-flow__handle.source[data-handleid="timed_out"]')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /Add node after 同意|在同意后添加节点/i }))
    expect(onQuickAdd).toHaveBeenCalledWith('node-approval', 'decision:ok', 'decision')
    expect(node).toHaveStyle({ height: '144px' })
  })

  it('stacks multi-input rows with left handles and keeps the merge mode summary', () => {
    const value = manifest('merge')
    value.nodeType = value.displayName = 'merge'
    value.inputPorts = [
      { name: 'main', kind: 'main', required: false, variadic: false },
      { name: 'left', kind: 'main', required: false, variadic: false },
      { name: 'right', kind: 'main', required: false, variadic: false },
    ]
    canvasStore.setState({ manifests: new Map([['merge@1', value]]), runtimeStatuses: new Map(), bindingSummaries: new Map(), occupiedHandlesByNodeId: new Map(), zoomTier: 'full' })
    render(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...props('merge', { nodeType: 'merge', parameters: { mode: 'append' } })} /></ReactFlowProvider></CanvasRenderProvider>)
    const node = screen.getByTestId('studio-node-node-merge')
    expect(node.querySelectorAll('.studio-branch-row-input')).toHaveLength(0)
    expect(node.querySelector('.studio-card-row')).toHaveTextContent(/追加|Append/)
    expect(node.querySelector('.react-flow__handle.target[data-handleid="main"]')).toBeInTheDocument()
    for (const handleId of ['left', 'right']) expect(node.querySelector(`.react-flow__handle.target[data-handleid="${handleId}"]`)).not.toBeInTheDocument()
  })

  it('keeps branch row handles at compact zoom while row text degrades away', () => {
    const value = manifest('branch')
    value.nodeType = value.displayName = 'if'
    value.outputPorts = [
      { name: 'case', kind: 'main', required: false, variadic: true },
      { name: 'else', kind: 'main', required: false, variadic: false },
      { name: 'error', kind: 'error', required: false, variadic: false },
    ]
    canvasStore.setState({ manifests: new Map([['if@1', value]]), runtimeStatuses: new Map(), bindingSummaries: new Map(), occupiedHandlesByNodeId: new Map(), zoomTier: 'compact' })
    render(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...props('branch', { nodeType: 'if', parameters: { cases: [{ id: 'c1', conditions: [{ label: '金额 ≥ 1000' }], logicalOp: 'and' }] } })} /></ReactFlowProvider></CanvasRenderProvider>)
    const node = screen.getByTestId('studio-node-node-branch')
    expect(node).toHaveClass('studio-node-zoom-compact')
    expect(node.querySelectorAll('.studio-branch-row')).toHaveLength(2)
    expect(node.querySelectorAll('.studio-branch-text')).toHaveLength(2)
    expect(node.querySelector('.react-flow__handle.source[data-handleid="case:c1"]')).toBeInTheDocument()
    expect(node.querySelector('.react-flow__handle.source[data-handleid="else"]')).toBeInTheDocument()
    expect(node.querySelector('.react-flow__handle.source[data-handleid="error"]')).toBeInTheDocument()
  })

  it('keeps handles interactive at low zoom and shows actual Agent bindings', () => {
    const onSourceHover = vi.fn()
    canvasStore.setState({
      manifests: new Map([['agent@2', manifest('agent')]]), runtimeStatuses: new Map(),
      bindingSummaries: new Map([['node-agent', [{ role: 'mcp_tools', resourceType: 'mcp_tool', label: 'Weather tool' }]]]),
      occupiedHandlesByNodeId: new Map(),
      zoomTier: 'compact', onQuickAdd: () => undefined, onSourceHover,
    })
    render(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...props('agent')} /></ReactFlowProvider></CanvasRenderProvider>)
    const node = screen.getByTestId('studio-node-node-agent')
    expect(node).toHaveClass('studio-node-zoom-compact')
    expect(screen.getByTitle(/Weather tool/)).toBeInTheDocument()
    expect(node.querySelectorAll('.react-flow__handle')).toHaveLength(2)
    expect(node.querySelector('[data-handleid^="binding:"]')).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Add node after error|在错误后添加节点/i })).toBeInTheDocument()
    const source = node.querySelector('.react-flow__handle.source')!
    fireEvent.mouseEnter(source)
    fireEvent.mouseLeave(source)
    expect(onSourceHover).toHaveBeenNthCalledWith(1, 'node-agent', 'error', true)
    expect(onSourceHover).toHaveBeenNthCalledWith(2, 'node-agent', 'error', false)
  })

  it('places error outputs on the right below normal outputs', () => {
    const value = manifest('default')
    value.outputPorts = [
      { name: 'error', kind: 'error', required: false, variadic: false },
      { name: 'main', kind: 'main', required: false, variadic: false },
    ]
    canvasStore.setState({ manifests: new Map([['default@1', value]]), runtimeStatuses: new Map(), bindingSummaries: new Map(), occupiedHandlesByNodeId: new Map(), zoomTier: 'full' })
    render(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...props('default')} /></ReactFlowProvider></CanvasRenderProvider>)
    const node = screen.getByTestId('studio-node-node-default')
    const main = node.querySelector<HTMLElement>('.react-flow__handle.source[data-handleid="main"]')!
    const error = node.querySelector<HTMLElement>('.react-flow__handle.source[data-handleid="error"]')!
    expect(main).toHaveClass('react-flow__handle-right')
    expect(error).toHaveClass('react-flow__handle-right')
    expect(Number.parseFloat(error.style.top)).toBeGreaterThan(Number.parseFloat(main.style.top))
    const mainLabel = node.querySelector<HTMLElement>('[data-port-type="source"][data-port-id="main"]')!
    const errorLabel = node.querySelector<HTMLElement>('[data-port-type="source"][data-port-id="error"]')!
    expect(mainLabel.style.top).toBe(main.style.top)
    expect(errorLabel.style.top).toBe(error.style.top)
    expect(main).toHaveClass('!grid', '!place-items-center')
    expect(main.style.top).toBe('24px')
    expect(error.style.top).toBe('52px')
  })

  it('keeps quick add visible only for unoccupied or repeatable ports', () => {
    const single = manifest('agent')
    canvasStore.setState({
      manifests: new Map([['agent@2', single]]), runtimeStatuses: new Map(), bindingSummaries: new Map(),
      occupiedHandlesByNodeId: new Map([['node-agent', 'error']]), zoomTier: 'full', onQuickAdd: () => undefined,
    })
    render(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><ManifestNode {...props('agent')} /></ReactFlowProvider></CanvasRenderProvider>)
    const node = screen.getByTestId('studio-node-node-agent')
    expect(node.querySelectorAll('.studio-port-add')).toHaveLength(0)

    const repeatable = manifest('agent')
    repeatable.outputPorts[0].variadic = true
    act(() => canvasStore.setState({ manifests: new Map([['agent@2', repeatable]]) }))
    expect(node.querySelectorAll('.studio-port-add')).toHaveLength(1)
  })

  it('rerenders only the node whose runtime status changes', () => {
    const value = manifest('default')
    canvasStore.setState({ manifests: new Map([['default@1', value]]), runtimeStatuses: new Map(), bindingSummaries: new Map(), occupiedHandlesByNodeId: new Map(), zoomTier: 'full' })
    const first = vi.fn()
    const second = vi.fn()
    render(<CanvasRenderProvider store={canvasStore}><ReactFlowProvider><Profiler id="first" onRender={first}><ManifestNode {...props('default')} id="first" /></Profiler><Profiler id="second" onRender={second}><ManifestNode {...props('default')} id="second" /></Profiler></ReactFlowProvider></CanvasRenderProvider>)
    first.mockClear()
    second.mockClear()

    act(() => canvasStore.setState({ runtimeStatuses: new Map([['first', 'running']]) }))
    expect(first).toHaveBeenCalledOnce()
    expect(second).not.toHaveBeenCalled()
  })
})
