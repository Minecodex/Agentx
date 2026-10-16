import { useDesignOnly } from "../model/design-effects"
import * as React from 'react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useTheme } from '../../../app/providers/theme-provider'

import { installPluginStyles, loadPluginUi, PluginUiBoundary } from '../../../shared/plugin-ui-loader'
import { Input } from '../../../shared/ui/input'
import { Button } from '../../../shared/ui/button'
import { Select } from '../../../shared/ui/select'
import type { NodeManifest } from '../model/types'

export function PluginCanvas({ manifest, parameters }: { manifest: NodeManifest; parameters: Record<string, unknown> }) {
  const designOnly = useDesignOnly()
  const { i18n } = useTranslation()
  const { resolvedTheme } = useTheme()
  const [Canvas, setCanvas] = useState<React.ComponentType<{ parameters: Record<string, unknown> }>>()
  useEffect(() => {
    let active = true
    if (designOnly) return () => { active = false }
    const source = manifest.plugin?.uiSource
    if (!source) return () => { active = false }
    const removeStyles = installPluginStyles(manifest.plugin?.uiStyles, manifest.plugin?.bundleDigest ?? manifest.nodeType)
    void loadPluginUi(source, manifest.plugin?.bundleDigest ?? manifest.nodeType).then((module) => {
      const ui = module.createUi({ React, locale: i18n.language, theme: resolvedTheme, portalRoot: document.body, assets: manifest.plugin?.uiAssets ?? {}, components: { Input, Button, Select, SmartInput: Input, Field: ({ label, children }: { label: string; children?: React.ReactNode }) => <label><span>{label}</span>{children}</label> } })
      if (active) setCanvas(() => ui.Canvas)
    }).catch(() => undefined)
    return () => { active = false; removeStyles() }
  }, [designOnly, i18n.language, manifest.nodeType, manifest.version, manifest.plugin?.bundleDigest, manifest.plugin?.uiSource, manifest.plugin?.uiStyles, manifest.plugin?.uiAssets, resolvedTheme])
  if (designOnly) return null
  return Canvas ? <PluginUiBoundary fallback={null}><div className="min-h-0 px-3 pb-2 text-[10px]"><Canvas parameters={parameters} /></div></PluginUiBoundary> : null
}
