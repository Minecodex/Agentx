import { memo } from 'react'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

/** Read-only prose, including incomplete streaming Markdown. Raw HTML is never executed. */
export const MarkdownContent = memo(function MarkdownContent({ children }: { children: string }) {
  return <div className="agentx-markdown-prose min-w-0 break-words leading-relaxed" data-testid="markdown-content">
    <Markdown remarkPlugins={[remarkGfm]} skipHtml components={{
      a: ({ children, href }) => <a href={href} rel="noopener noreferrer" target="_blank">{children}</a>,
      table: ({ children }) => <div className="max-w-full overflow-x-auto"><table>{children}</table></div>,
    }}>{children}</Markdown>
  </div>
})
