import { Copy, Terminal } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { runtimePublicBaseUrl } from '../../../shared/api/client'
import { JsonSchemaViewer } from '../../../shared/components/json-schema-viewer'
import { Button } from '../../../shared/ui/button'
import { Card } from '../../../shared/ui/card'
import { useToast } from '../../../shared/ui/toast'
import type { McpRevision } from './application-mcp-tab'
import { McpClients } from './mcp-clients'
import { McpDiagnostics } from './mcp-diagnostics'

export function McpDocumentation({ applicationId, revision }: { applicationId: string; revision: McpRevision }) {
  const { t } = useTranslation()
  const { showToast } = useToast()
  const publication = revision.publication
  const url = `${runtimePublicBaseUrl()}/gateway/v1/mcp/applications/${publication.slug}`
  const copy = async (value: string) => { await navigator.clipboard.writeText(value); showToast(t('applicationMcp.copied')) }
  return <div className="space-y-5" data-testid="mcp-documentation">
    <Card className="space-y-5 p-5">
      <div className="flex items-start justify-between gap-6"><div><div className="flex items-center gap-2"><Terminal className="size-4 text-primary" /><h3 className="text-sm font-semibold">{publication.toolName}</h3><span className="rounded bg-success/10 px-2 py-0.5 text-[11px] text-success">{t('applicationMcp.active')}</span><span className="text-xs text-muted-foreground">#{publication.revision}</span></div><p className="mt-2 text-sm text-muted-foreground">{publication.description}</p></div></div>
      <div><h4 className="mb-2 text-xs font-medium">{t('applicationMcp.endpoint')}</h4><div className="flex items-center gap-3 rounded-lg border border-border bg-canvas px-3 py-2"><code className="min-w-0 flex-1 break-all text-xs">{url}</code><Button size="sm" variant="ghost" aria-label={t('applicationMcp.copyEndpoint')} onClick={() => void copy(url)}><Copy className="size-3.5" /></Button></div></div>
      <div className="grid grid-cols-2 gap-5"><div><h4 className="mb-2 text-xs font-medium">{t('mcp.inputSchema')}</h4><JsonSchemaViewer schema={publication.inputSchema} /></div><div><h4 className="mb-2 text-xs font-medium">{t('mcp.outputSchema')}</h4><JsonSchemaViewer schema={publication.outputSchema} /></div></div>
      <details className="rounded-lg border border-border p-3 text-xs"><summary className="cursor-pointer font-medium">{t('applicationMcp.protocol')} · {publication.protocolVersion}</summary><p className="mt-3 leading-5 text-muted-foreground">{t('applicationMcp.helpers')}</p><code className="mt-2 block">get_task · submit_task_input · cancel_task</code><p className="mt-3 leading-5 text-muted-foreground">{t('applicationMcp.revisionHint', { revision: publication.revision })}</p></details>
    </Card>
    <McpClients key={publication.publicationId} applicationId={applicationId} publicationId={publication.publicationId} toolName={publication.toolName} url={url} />
    <McpDiagnostics key={publication.publicationId} applicationId={applicationId} publicationId={publication.publicationId} />
  </div>
}
