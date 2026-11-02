import { ExternalLink, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { ConfirmDialog } from '../../../shared/components/confirm-dialog'
import { useLocaleFormat } from '../../../shared/lib/locale-format'
import { Button } from '../../../shared/ui/button'
import { Dialog, DialogContent } from '../../../shared/ui/dialog'
import type { McpRevision } from './application-mcp-tab'

export function McpPublicationHistory({ rows, removing, error, onClose, onSelect, onRemove }: {
  rows: McpRevision[]; removing: boolean; error?: string; onClose: () => void
  onSelect: (id: string) => void; onRemove: (id: string) => Promise<void>
}) {
  const { t } = useTranslation()
  const { formatDateTime } = useLocaleFormat()
  const [target, setTarget] = useState<McpRevision>()
  const visible = rows.filter(item => !item.deleted || item.deletionStatus !== 'published')
  return <>
    <Dialog open onOpenChange={open => { if (!open && !removing) onClose() }}>
      <DialogContent className="w-[min(1080px,calc(100vw-48px))] p-6" title={t('applicationMcp.history')}>
        <h2 className="text-lg font-semibold">{t('applicationMcp.history')}</h2>
        <p className="mt-2 text-xs text-muted-foreground">{t('applicationMcp.historyHint')}</p>
        <table className="mt-5 w-full text-left text-xs"><thead><tr className="border-b border-border text-muted-foreground">
          {[t('applicationMcp.version'), t('applicationMcp.toolName'), t('applicationMcp.slug'), t('applicationMcp.purpose'), t('applicationMcp.publishedAt'), t('common.status'), t('common.actions')].map(title => <th className="whitespace-nowrap px-3 py-3 font-medium" key={title}>{title}</th>)}
        </tr></thead><tbody>{visible.map(item => <tr className="border-b border-border/60" key={item.publication.publicationId} data-testid={`mcp-publication-${item.publication.revision}`}>
          <td className="px-3 py-4">#{item.publication.revision}</td>
          <td className="px-3 py-4 font-mono">{item.publication.toolName}</td>
          <td className="px-3 py-4 font-mono">/{item.publication.slug}</td>
          <td className="max-w-64 px-3 py-4"><p className="line-clamp-2" title={item.publication.description}>{item.publication.description}</p></td>
          <td className="whitespace-nowrap px-3 py-4 text-muted-foreground">{formatDateTime(item.createdAt)}</td>
          <td className="px-3 py-4"><span className={`whitespace-nowrap ${item.status === 'published' && !item.deleted ? 'text-success' : 'text-muted-foreground'}`}>{t(item.deleted ? 'applicationMcp.deleting' : item.status === 'published' ? 'applicationMcp.active' : 'applicationMcp.pending')}</span>{item.error && <p className="mt-1 max-w-48 text-danger">{item.error}</p>}</td>
          <td className="px-3 py-4"><div className="flex items-center gap-1"><Button size="sm" variant="ghost" disabled={item.deleted || item.status !== 'published'} aria-label={`${t('applicationMcp.viewDocumentation')} #${item.publication.revision}`} onClick={() => onSelect(item.publication.publicationId)}><ExternalLink className="size-3.5" /></Button><Button size="sm" variant="ghost" disabled={removing || item.deleted || item.status !== 'published'} aria-label={`${t('applicationMcp.deletePublication')} #${item.publication.revision}`} onClick={() => setTarget(item)}><Trash2 className="size-3.5 text-danger" /></Button></div></td>
        </tr>)}</tbody></table>
        {!visible.length && <p className="py-8 text-center text-sm text-muted-foreground">{t('applicationMcp.historyEmpty')}</p>}
        {error && <p className="mt-3 text-xs text-danger" role="alert">{error}</p>}
        <div className="mt-5 flex justify-end"><Button variant="secondary" onClick={onClose}>{t('common.close')}</Button></div>
      </DialogContent>
    </Dialog>
    {target && <ConfirmDialog open pending={removing} title={t('applicationMcp.deletePublication')} description={t('applicationMcp.deleteHint', { name: target.publication.toolName, slug: target.publication.slug, revision: target.publication.revision })} cancelLabel={t('common.cancel')} confirmLabel={t('common.delete')} onClose={() => setTarget(undefined)} onConfirm={async () => { try { await onRemove(target.publication.publicationId) } catch { /* The mutation error is displayed in history. */ } finally { setTarget(undefined) } }} />}
  </>
}
