import { ReactFlowProvider, type NodeProps } from '@xyflow/react'
import { render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'

import { i18n } from '../../../app/i18n'
import type { CanvasNode } from '../model/types'
import { BoundaryNode } from './boundary-node'

function props(boundary: 'start' | 'end'): NodeProps<CanvasNode> {
  return {
    id: `__${boundary}__`,
    type: 'boundary',
    data: { editorKind: 'boundary', boundary, label: '' },
    selected: false,
  } as unknown as NodeProps<CanvasNode>
}

function renderBoundary(boundary: 'start' | 'end') {
  return render(<ReactFlowProvider><BoundaryNode {...props(boundary)} /></ReactFlowProvider>)
}

afterEach(async () => {
  await i18n.changeLanguage('zh-CN')
})

describe('BoundaryNode', () => {
  it('labels the Start input port in Chinese', async () => {
    await i18n.changeLanguage('zh-CN')
    renderBoundary('start')
    expect(screen.getByText('开始')).toBeInTheDocument()
    expect(screen.getByText('工作流入口')).toBeInTheDocument()
    const node = screen.getByTestId('workflow-start')
    expect(node).toHaveStyle({ width: '184px', height: '80px' })
    expect(node.querySelector('.studio-card')).toBeInTheDocument()
    expect(screen.getByTestId('boundary-port-main')).toHaveAttribute('aria-label', '输入')
    expect(screen.getByTestId('boundary-port-main')).toHaveStyle({ top: '24px' })
  })

  it('separates and labels the End output and error ports', async () => {
    await i18n.changeLanguage('en-US')
    renderBoundary('end')
    const output = screen.getByTestId('boundary-port-main')
    const error = screen.getByTestId('boundary-port-error')
    expect(screen.getByText('End')).toBeInTheDocument()
    expect(output).toHaveAttribute('aria-label', 'Output')
    expect(error).toHaveAttribute('aria-label', 'Error')
    expect(output.style.top).toBe('24px')
    expect(error.style.top).toBe('52px')
    expect(screen.getByTestId('boundary-port-main').style.top).toBe(output.style.top)
    expect(screen.getByTestId('boundary-port-error').style.top).toBe(error.style.top)
  })
})
