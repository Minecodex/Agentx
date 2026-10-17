import * as Popover from '@radix-ui/react-popover'
import { ChevronDown, Search, ShieldCheck } from 'lucide-react'
import { Fragment, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router-dom'

import { scopeLabel } from '../../knowledge/scope'
import { Button } from '../../../shared/ui/button'
import { localizedValue } from '../../../shared/lib/localized-value'
import { Dialog, DialogContent } from '../../../shared/ui/dialog'
import { Input } from '../../../shared/ui/input'
import type { ResourceOption } from '../model/types'

type ResourceAction = { option: ResourceOption; kind: 'grant' | 'request' }

export function ResourcePicker({ options, value, onChange, onAuthorize, onRequest, ariaLabel }: {
  options: ResourceOption[]
  value?: string
  onChange: (id: string, versionId?: string | null, label?: string) => void
  onAuthorize?: (option: ResourceOption) => Promise<void>
  onRequest?: (option: ResourceOption, message?: string) => Promise<void>
  ariaLabel?: string
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const [search, setSearch] = useState('')
  const [action, setAction] = useState<ResourceAction>()
  const [message, setMessage] = useState('')
  const [pending, setPending] = useState(false)
  const selected = options.find((item) => item.value === value)
  const selectedUnavailable = Boolean(value && (!selected || (selected.accessState !== undefined && selected.accessState !== 'authorized')))
  const visibleOptions = useMemo(() => {
    const normalized = search.trim().toLocaleLowerCase()
    const sorted = [...options].sort((a, b) => (a.metadata?.connectionName ?? '').localeCompare(b.metadata?.connectionName ?? '') || (a.metadata?.connectionId ?? '').localeCompare(b.metadata?.connectionId ?? '') || a.label.localeCompare(b.label))
    if (!normalized) return sorted
    return sorted.filter((option) => `${option.label} ${option.detail ?? ''} ${option.metadata?.connectionName ?? ''} ${JSON.stringify(option.metadata?.scope ?? '')}`.toLocaleLowerCase().includes(normalized))
  }, [options, search])

  const beginAction = (next: ResourceAction) => {
    setOpen(false)
    setAction(next)
  }
  const confirmAction = async () => {
    if (!action) return
    setPending(true)
    try {
      if (action.kind === 'grant') await onAuthorize?.(action.option)
      else await onRequest?.(action.option, message.trim() || undefined)
      setAction(undefined)
      setMessage('')
    } finally {
      setPending(false)
    }
  }

  return <>
    <Popover.Root onOpenChange={(next) => { setOpen(next); if (!next) setSearch('') }} open={open}>
      <Popover.Trigger asChild>
        <button aria-expanded={open} aria-haspopup="listbox" aria-invalid={selectedUnavailable || undefined} aria-label={ariaLabel ?? (selectedUnavailable ? t('studio.inspector.selectedResourceUnavailable') : selected?.label ?? t('studio.inspector.selectResource'))} className={`inline-flex h-9 w-full items-center justify-between gap-3 rounded-lg border bg-surface px-3 text-left text-xs outline-none transition-colors hover:bg-muted/45 focus:ring-2 ${selectedUnavailable ? 'border-danger/60 text-danger focus:ring-danger/15' : 'border-border text-foreground focus:border-primary/60 focus:ring-primary/15'}`} role="combobox" type="button">
          <span className={selected || selectedUnavailable ? 'truncate' : 'truncate text-muted-foreground'}>{selectedUnavailable ? t('studio.inspector.selectedResourceUnavailable') : selected?.label ?? t('studio.inspector.selectResource')}</span>
          <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content align="start" className="z-[220] max-h-80 w-[var(--radix-popover-trigger-width)] overflow-hidden rounded-lg border border-border bg-surface shadow-xl" sideOffset={4}>
          <div className="relative border-b border-border p-2">
            <Search className="pointer-events-none absolute left-4 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input aria-label={t('common.search')} className="h-8 pl-8 text-xs" onChange={(event) => setSearch(event.target.value)} value={search} />
          </div>
          <div className="max-h-64 overflow-y-auto p-1" role="listbox">
            {visibleOptions.length === 0 && <p className="p-3 text-[11px] text-muted-foreground">{t('studio.inspector.noResource')}</p>}
            {visibleOptions.map((option, index) => {
              const selectable = option.accessState === 'authorized' || option.accessState === undefined
              const actionKind = option.accessState === 'grantable' ? 'grant' : option.accessState === 'requestable' || option.accessState === 'rejected' ? 'request' : undefined
              return <Fragment key={option.value}>{option.metadata?.connectionName && visibleOptions[index - 1]?.metadata?.connectionId !== option.metadata.connectionId && <p className="px-2 pb-1 pt-3 text-[11px] font-semibold text-muted-foreground">{option.metadata.connectionName}</p>}<div className="flex min-h-11 items-center gap-2 rounded-md px-2 py-1.5 hover:bg-muted/45" key={option.value}>
                <button aria-disabled={!selectable} className={`min-w-0 flex-1 truncate text-left text-xs ${selectable ? 'text-foreground' : 'cursor-not-allowed text-muted-foreground'}`} disabled={!selectable} onClick={() => { onChange(option.value, option.versionId, option.label); setOpen(false) }} role="option" type="button">
                  <span className="block truncate">{option.label}</span>
                  {option.detail && <span className="block truncate text-[10px] text-muted-foreground">{option.metadata?.description ?? option.detail}</span>}
                  {option.metadata?.scope && <span className="block truncate text-[10px] text-muted-foreground">{scopeLabel(option.metadata.scope, t('knowledge.defaultWorkspace'))}</span>}
                </button>
                {option.accessState && option.accessState !== 'authorized' && <span className="shrink-0 text-[10px] text-muted-foreground">{localizedValue(t, 'studio.inspector.resourceStates', option.accessState)}</span>}
                {actionKind === 'grant' && onAuthorize && <Button onClick={() => beginAction({ option, kind: 'grant' })} size="sm" variant="ghost"><ShieldCheck className="size-3.5" />{t('studio.inspector.authorize')}</Button>}
                {actionKind === 'request' && onRequest && <Button onClick={() => beginAction({ option, kind: 'request' })} size="sm" variant="ghost"><ShieldCheck className="size-3.5" />{t(option.accessState === 'rejected' ? 'studio.inspector.requestAgain' : 'studio.inspector.request')}</Button>}
                {option.accessState === 'pending' && option.pendingRequestId && <Button asChild size="sm" variant="ghost"><Link to={`/approvals/resource-grants/${option.pendingRequestId}`}>{t('studio.inspector.viewRequest')}</Link></Button>}
              </div></Fragment>
            })}
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
    <Dialog onOpenChange={(next) => { if (!next && !pending) { setAction(undefined); setMessage('') } }} open={Boolean(action)}>
      <DialogContent title={t(action?.kind === 'grant' ? 'studio.inspector.authorizeTitle' : 'studio.inspector.requestTitle')}>
        <div className="space-y-4 p-5">
          <p className="text-xs leading-5 text-muted-foreground">{t(action?.kind === 'grant' ? 'studio.inspector.authorizeDescription' : 'studio.inspector.requestDescription')}</p>
          <div className="space-y-2 rounded-lg border border-border bg-muted/30 p-3">
            {action?.option.requirements?.map((requirement) => <div className="flex items-center justify-between gap-3 text-xs" key={`${requirement.resourceType}:${requirement.resourceId}`}><span className="truncate">{requirement.name ?? t('approvals.controlledDependency')}</span><span className="shrink-0 text-muted-foreground">{localizedValue(t, 'resourceGrants.operations', requirement.operation)}</span></div>)}
          </div>
          {action?.kind === 'request' && <Input aria-label={t('studio.inspector.requestMessage')} onChange={(event) => setMessage(event.target.value)} placeholder={t('studio.inspector.requestMessagePlaceholder')} value={message} />}
          <div className="flex justify-end gap-2"><Button onClick={() => setAction(undefined)} variant="ghost">{t('common.cancel')}</Button><Button disabled={pending} onClick={() => void confirmAction()}>{pending ? t('common.loading') : t(action?.kind === 'grant' ? 'studio.inspector.confirmAuthorize' : 'studio.inspector.confirmRequest')}</Button></div>
        </div>
      </DialogContent>
    </Dialog>
  </>
}
