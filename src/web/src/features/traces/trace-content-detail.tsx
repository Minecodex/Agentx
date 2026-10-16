import { Download } from 'lucide-react'
import * as React from 'react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useTheme } from '../../app/providers/theme-provider'

import { apiRequest } from '../../shared/api/client'
import type { TraceContent, TraceSpanDetail } from '../../shared/api/types'
import { installPluginStyles, loadPluginUi, PluginUiBoundary } from '../../shared/plugin-ui-loader'
import { Button } from '../../shared/ui/button'
import { Input } from '../../shared/ui/input'
import { Select } from '../../shared/ui/select'
import { JsonBlock, TraceSemanticValue } from './trace-semantic-value'
import { useDesignOnly } from '../workflow-designer/model/design-effects'

export type ContentKind = TraceContent['kind']

export function TraceContents({ contents, executionId, advanced = false, onDownloadArtifact }: { contents: TraceContent[]; executionId?: string; advanced?: boolean; onDownloadArtifact?: (artifactId: string) => void }) {
  const { t } = useTranslation()
  if (!contents.length) return <p className="text-xs text-muted-foreground">{t('trace.noDiagnosticContent')}</p>
  const groups = groupContents(contents)
  return <div className="space-y-4">{groups.map(([kind, entries]) => <section className="space-y-2" key={kind}>
    <h4 className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">{t(`trace.contentKinds.${kind}`)}</h4>
    {entries.map((entry) => <div className="space-y-2 rounded-lg border border-border p-3" key={entry.eventId}>
      {advanced ? <JsonBlock value={entry.preview} /> : entry.kind === 'plugin_content' ? <PluginTraceValue executionId={executionId} value={entry.preview} /> : <TraceSemanticValue value={entry.preview} />}
      {entry.contentRef && onDownloadArtifact && <Button onClick={() => onDownloadArtifact(entry.contentRef!)} size="sm" variant="secondary"><Download className="size-3.5" />{t('trace.downloadArtifact')}</Button>}
    </div>)}
  </section>)}</div>
}

type PluginTraceEnvelope = { nodeType?: string; typeVersion?: number; bundleDigest?: string; contentType?: string; contentVersion?: number; data?: unknown }

function PluginTraceValue({ value, executionId }: { value: unknown; executionId?: string }) {
  const designOnly = useDesignOnly()
  const { i18n, t } = useTranslation()
  const { resolvedTheme } = useTheme()
  const envelope = (value ?? {}) as PluginTraceEnvelope
  const [Renderer, setRenderer] = useState<React.ComponentType<{ value: unknown }>>()
  const [fallback, setFallback] = useState(false)
  useEffect(() => {
    let active = true
    let removeStyles: () => void = () => undefined
    setRenderer(undefined)
    setFallback(false)
    if (designOnly) return () => { active = false }
    if (!envelope.nodeType || !envelope.typeVersion || !envelope.bundleDigest || !envelope.contentType) {
      setFallback(true)
      return () => { active = false }
    }
    const path = executionId
      ? `/executions/${executionId}/plugin-manifests/${encodeURIComponent(envelope.nodeType)}/versions/${envelope.typeVersion}?bundleDigest=${encodeURIComponent(envelope.bundleDigest)}`
      : `/node-definitions/${encodeURIComponent(envelope.nodeType)}/versions/${envelope.typeVersion}?bundleDigest=${encodeURIComponent(envelope.bundleDigest)}`
    void apiRequest<{ manifest: { plugin?: { uiSource?: string | null; uiStyles?: string | null; uiAssets?: Record<string, string>; traceRenderers?: Array<{ contentType: string; contentVersion: number; exportName: string }> } } }>(path)
      .then(async ({ manifest }) => {
        const source = manifest.plugin?.uiSource
        const spec = manifest.plugin?.traceRenderers?.find((item) => item.contentType === envelope.contentType && item.contentVersion === envelope.contentVersion)
        if (!source || !spec) { if (active) setFallback(true); return }
        removeStyles = installPluginStyles(manifest.plugin?.uiStyles, envelope.bundleDigest!)
        const module = await loadPluginUi(source, envelope.bundleDigest!)
        const ui = module.createUi({ React, locale: i18n.language, theme: resolvedTheme, portalRoot: document.body, assets: manifest.plugin?.uiAssets ?? {}, components: { Input, Button, Select, SmartInput: Input, Field: ({ label, children }: { label: string; children?: React.ReactNode }) => <label className="block text-xs"><span>{label}</span>{children}</label> } })
        const renderer = ui.traceRenderers?.[spec.exportName]
        if (active) { setRenderer(() => renderer); setFallback(!renderer) } else removeStyles()
      }).catch(() => { if (active) setFallback(true) })
    return () => { active = false; removeStyles() }
  }, [designOnly, envelope.bundleDigest, envelope.contentType, envelope.contentVersion, envelope.nodeType, envelope.typeVersion, executionId, i18n.language, resolvedTheme])
  return <div className="space-y-2">
    {fallback && <p className="text-[10px] text-muted-foreground">{t('trace.pluginRendererUnavailable')}</p>}
    {!designOnly && Renderer ? <PluginUiBoundary><Renderer value={envelope.data} /></PluginUiBoundary> : <TraceSemanticValue value={envelope.data ?? value} />}
  </div>
}

export function contentPreview(detail: TraceSpanDetail | undefined, kind: ContentKind) {
  return detail?.contents?.find((content) => content.kind === kind)?.preview
}

export function contentsForKinds(contents: TraceContent[], kinds: ContentKind[]) {
  const selected = new Set(kinds)
  return contents.filter((content) => selected.has(content.kind))
}

export function groupContents(contents: TraceContent[]) {
  const groups = new Map<ContentKind, TraceContent[]>()
  for (const content of contents) {
    const group = groups.get(content.kind) ?? []
    group.push(content)
    groups.set(content.kind, group)
  }
  return [...groups.entries()]
}
