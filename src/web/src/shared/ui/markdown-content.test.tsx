import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { MarkdownContent } from './markdown-content'

describe('MarkdownContent', () => {
  it('renders assistant prose, lists, code, tables and streaming updates', () => {
    const view = render(<MarkdownContent>{'## Workflow\n\n- **Model** uses `question`\n\n```json\n{"ok": true}'}</MarkdownContent>)
    expect(screen.getByRole('heading')).toHaveTextContent('Workflow')
    expect(screen.getByRole('listitem').querySelector('strong')).toHaveTextContent('Model')
    expect(view.container.querySelector('pre code')).toHaveTextContent('{"ok": true}')
    view.rerender(<MarkdownContent>{'| Node | Status |\n| --- | --- |\n| Model | Ready |'}</MarkdownContent>)
    expect(screen.getByRole('table')).toHaveTextContent('ModelReady')
  })
  it('does not execute model HTML or unsafe links', () => {
    const { container } = render(<MarkdownContent>{'<script>alert(1)</script>\n\n[x](javascript:alert%281%29) [docs](https://example.com)'}</MarkdownContent>)
    expect(container.querySelector('script')).toBeNull()
    expect(screen.getByText('x')).not.toHaveAttribute('href', expect.stringContaining('javascript:'))
    expect(screen.getByRole('link', { name: 'docs' })).toHaveAttribute('rel', 'noopener noreferrer')
  })
})
