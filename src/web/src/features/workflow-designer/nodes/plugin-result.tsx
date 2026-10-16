import { useDesignOnly } from "../model/design-effects"
import * as React from 'react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useTheme } from '../../../app/providers/theme-provider'

import { Button } from '../../../shared/ui/button'
import { Input } from '../../../shared/ui/input'
import { Select } from '../../../shared/ui/select'
import { installPluginStyles, loadPluginUi, PluginUiBoundary } from '../../../shared/plugin-ui-loader'
import type { NodeManifest } from '../model/types'

export function PluginResult({ manifest, value }: { manifest: NodeManifest; value: unknown }) {
  const designOnly = useDesignOnly()
  const { i18n } = useTranslation()
  const { resolvedTheme } = useTheme()
  const [Result, setResult] = useState<React.ComponentType<{ value: unknown }>>()
  const [failed, setFailed] = useState(false)
  useEffect(() => {
    let active = true
    if (designOnly) return () => { active = false }
    const source = manifest.plugin?.uiSource
    if (!source) return () => { active = false }
    const removeStyles = installPluginStyles(manifest.plugin?.uiStyles, manifest.plugin?.bundleDigest ?? manifest.nodeType)
    void loadPluginUi(source, manifest.plugin?.bundleDigest ?? manifest.nodeType)
      .then((module) => {
        const ui = module.createUi({ React, locale: i18n.language, theme: resolvedTheme, portalRoot: document.body, assets: manifest.plugin?.uiAssets ?? {}, components: { Input, Button, Select, SmartInput: Input, Field: ({ label, children }: { label: string; children?: React.ReactNode }) => <label><span>{label}</span>{children}</label> } })
        if (active) setResult(() => ui.Result)
      })
      .catch(() => { if (active) setFailed(true) })
    return () => { active = false; removeStyles() }
  }, [designOnly, i18n.language, manifest, resolvedTheme])
  if (designOnly) return null
  if (failed || !Result) return null
  return <PluginUiBoundary fallback={null}><div className="mb-3 rounded-lg border border-border p-3"><Result value={value} /></div></PluginUiBoundary>
}
