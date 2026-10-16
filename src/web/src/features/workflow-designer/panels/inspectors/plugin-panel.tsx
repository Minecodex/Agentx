import { useDesignOnly } from "../../model/design-effects"
import * as React from 'react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest } from '../../../../shared/api/client'
import { useTheme } from '../../../../app/providers/theme-provider'
import { Input } from '../../../../shared/ui/input'
import { installPluginStyles, loadPluginUi, PluginUiBoundary, type PluginPanelComponentProps } from '../../../../shared/plugin-ui-loader'
import { SmartInput, asInputBinding } from '../../forms/binding-inputs'
import { Button } from '../../../../shared/ui/button'
import { Select } from '../../../../shared/ui/select'
import { PanelField, ParameterControl, type ActionPanelProps } from './panel-shell'
import { resolveNodeDefinition } from '../../api/studio-api'

export function PluginPanel({ data, manifest, fieldErrors, providerOptions, referenceCatalog, resources, onChange, readOnly = false }: ActionPanelProps) {
  const designOnly = useDesignOnly()
  const { i18n } = useTranslation()
  const { resolvedTheme } = useTheme()
  const [Panel, setPanel] = useState<React.ComponentType<PluginPanelComponentProps>>()
  const [error, setError] = useState('')
  const parametersRef = React.useRef(data.parameters)
  const referenceCatalogRef = React.useRef(referenceCatalog)
  const resourceReferencesRef = React.useRef(data.resourceReferences)
  parametersRef.current = data.parameters
  referenceCatalogRef.current = referenceCatalog
  resourceReferencesRef.current = data.resourceReferences
  useEffect(() => {
    let active = true
    if (designOnly) return () => { active = false }
    const source = manifest.plugin?.uiSource
    if (!source) { setError('Plugin UI source is missing'); return () => { active = false } }
    const removeStyles = installPluginStyles(manifest.plugin?.uiStyles, manifest.plugin?.bundleDigest ?? manifest.nodeType)
    void loadPluginUi(source, manifest.plugin?.bundleDigest ?? manifest.nodeType)
      .then((module) => {
        const ui = module.createUi({
          React,
          locale: i18n.language,
          theme: resolvedTheme,
          portalRoot: document.body,
          assets: manifest.plugin?.uiAssets ?? {},
          components: {
            Input, Button, Select,
            Field: ({ label, error: fieldError, children }: { label: string; error?: string; children: React.ReactNode }) => <PanelField error={fieldError} fieldPath={label} label={label}>{children}</PanelField>,
            SmartInput: ({ value, onChange, schema }: { value: unknown; onChange: (value: unknown) => void; schema?: Record<string, unknown> }) => <SmartInput catalog={referenceCatalogRef.current} expectedSchema={schema} onChange={onChange} value={asInputBinding(value)} />,
          },
          design: {
            resolveDefinition: (configuration: Record<string, unknown>, upstreamContracts: Record<string, unknown> = {}, signal?: AbortSignal) => resolveNodeDefinition(manifest.nodeType, manifest.version, configuration, upstreamContracts, signal),
            invokeProvider: (provider: string, input: { search?: string; limit?: number; cursor?: string; parameters?: Record<string, unknown> } = {}, signal?: AbortSignal) => {
              const query = new URLSearchParams()
              if (input.search) query.set('search', input.search)
              if (input.limit) query.set('limit', String(input.limit))
              if (input.cursor) query.set('cursor', input.cursor)
              query.set('parameters', JSON.stringify(input.parameters ?? parametersRef.current))
              query.set('resourceReferences', JSON.stringify(resourceReferencesRef.current))
              return apiRequest<{ items: unknown[]; nextCursor?: string | null }>(`/node-definitions/${encodeURIComponent(manifest.nodeType)}/versions/${manifest.version}/providers/${encodeURIComponent(provider)}?${query}`, { signal })
            },
          },
        })
        if (active) setPanel(() => ui.Panel)
      })
      .catch((reason: unknown) => { if (active) setError(reason instanceof Error ? reason.message : String(reason)) })
    return () => { active = false; removeStyles() }
  }, [designOnly, i18n.language, manifest.nodeType, manifest.plugin?.bundleDigest, manifest.plugin?.uiSource, manifest.plugin?.uiStyles, manifest.version, resolvedTheme])
  if (designOnly) return <div className="space-y-3">{Object.keys(manifest.parameterSchema.properties ?? {}).map(name => <ParameterControl key={name} name={name} panel={{data,manifest,fieldErrors,providerOptions,referenceCatalog,resources,onChange,readOnly}} />)}</div>
  if (error) return <div className="rounded-md border border-danger/30 bg-danger/5 p-3 text-xs text-danger" role="alert">{error}</div>
  if (!Panel) return <div className="p-3 text-xs text-muted-foreground">Loading plugin UI…</div>
  return <PluginUiBoundary><Panel fieldErrors={fieldErrors} parameters={data.parameters} providerOptions={providerOptions} readOnly={readOnly} referenceCatalog={referenceCatalog} resources={resources} updateParameters={(patch) => { if (!readOnly) onChange({ parameters: patch }) }} /></PluginUiBoundary>
}
