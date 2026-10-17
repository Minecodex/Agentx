import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { i18n } from '../../app/i18n'
import { JsonBlock, TraceSemanticValue } from './trace-semantic-value'

const report = '## 订单核算报告\n\n| 项目 | 金额 |\n| --- | --- |\n| **实付** | 558.00 |\n\n- 已核对 `order_result.json`'

describe('Trace semantic content', () => {
  beforeEach(async () => { await i18n.changeLanguage('zh-CN') })

  it('renders Markdown headings, tables, emphasis and code in port Items and scalar output', () => {
    const view = render(<TraceSemanticValue value={{ main: [{ json: { answer: report } }] }} />)
    expect(screen.getByRole('heading', { name: '订单核算报告' })).toBeInTheDocument()
    expect(screen.getByRole('table')).toHaveTextContent('实付558.00')
    expect(screen.getByText('实付').tagName).toBe('STRONG')
    expect(screen.getByText('order_result.json').tagName).toBe('CODE')
    view.rerender(<TraceSemanticValue value={report} />)
    expect(screen.getByRole('table')).toBeInTheDocument()
  })

  it('keeps unsafe Markdown inert while preserving execution-scoped downloads', () => {
    const download = vi.fn()
    const { container } = render(<TraceSemanticValue onDownloadArtifact={download} value={{
      answer: '<script>alert(1)</script>\n\n[危险](javascript:alert%281%29)',
      file: { artifactId: 'artifact-1' },
    }} />)
    expect(container.querySelector('script')).toBeNull()
    expect(container.querySelector('a')?.getAttribute('href')).toBeFalsy()
    fireEvent.click(screen.getByRole('button', { name: /Artifact/ }))
    expect(download).toHaveBeenCalledWith('artifact-1')
  })

  it('expands nested JSON on request and keeps JSON strings literal', () => {
    render(<JsonBlock value={{ nested: { enabled: true, count: 2, source: '**literal**', empty: null }, items: [] }} />)
    expect(screen.queryByText('true')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /nested/ }))
    expect(screen.getByText('true')).toHaveClass('text-primary')
    expect(screen.getByText('2')).toHaveClass('text-warning')
    expect(screen.getByText('"**literal**"')).toHaveClass('text-success')
    expect(screen.getByText('null')).toBeInTheDocument()
    expect(screen.getByText('[]')).toBeInTheDocument()
    expect(screen.queryByTestId('markdown-content')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /nested/ }))
    expect(screen.queryByText('true')).not.toBeInTheDocument()
  })
})
