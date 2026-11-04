import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Download, PackageOpen, Power, Star, Trash2 } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useParams } from 'react-router-dom'

import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest, apiRequestBlob, jsonBody } from '../../shared/api/client'
import { PageContainer } from '../../shared/components/page-container'
import { PageHeader } from '../../shared/components/page-header'
import { StatusBadge } from '../../shared/components/status-badge'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../shared/ui/tabs'
import { useToast } from '../../shared/ui/toast'
import { useLocaleFormat } from '../../shared/lib/locale-format'
import type { CanvasPlugin, CanvasPluginAuditItem, CanvasPluginReferences, CanvasPluginVersion } from './types'

export function CanvasPluginDetailPage() {
  const { id = '' } = useParams()
  const { t } = useTranslation()
  const auth = useAuth()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const { showToast } = useToast()
  const { formatDateTime } = useLocaleFormat()
  const plugin = useQuery({
    queryKey: ['canvas-plugin', id],
    queryFn: () => apiRequest<CanvasPlugin>(`/canvas-plugins/${id}`),
  })
  const references = useQuery({
    queryKey: ['canvas-plugin-references', id],
    queryFn: () => apiRequest<CanvasPluginReferences>(`/canvas-plugins/${id}/references`),
  })
  const audit = useQuery({
    queryKey: ['canvas-plugin-audit', id],
    queryFn: () => apiRequest<CanvasPluginAuditItem[]>(`/canvas-plugins/${id}/audit`),
    enabled: auth.hasPermission('canvas_plugin:manage'),
  })
  const refresh = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ['canvas-plugin', id] }),
      queryClient.invalidateQueries({ queryKey: ['canvas-plugins'] }),
      queryClient.invalidateQueries({ queryKey: ['node-definitions'] }),
    ])
  }
  const toggle = useMutation({
    mutationFn: (version: CanvasPluginVersion) => apiRequest<CanvasPlugin>(`/canvas-plugins/${id}/versions/${version.id}`, {
      method: 'PATCH', body: jsonBody({ enabled: version.status !== 'enabled', expectedRevision: plugin.data?.version }),
    }),
    onSuccess: refresh,
    onError: (error: Error) => showToast(error.message),
  })
  const setDefault = useMutation({
    mutationFn: (version: CanvasPluginVersion) => apiRequest<CanvasPlugin>(`/canvas-plugins/${id}`, {
      method: 'PATCH', body: jsonBody({ defaultVersionId: version.id, expectedRevision: plugin.data?.version }),
    }),
    onSuccess: refresh,
    onError: (error: Error) => showToast(error.message),
  })
  const removeVersion = useMutation({
    mutationFn: (version: CanvasPluginVersion) => apiRequest(`/canvas-plugins/${id}/versions/${version.id}`, { method: 'DELETE' }),
    onSuccess: refresh,
    onError: (error: Error) => showToast(error.message),
  })
  const uninstall = useMutation({
    mutationFn: () => apiRequest(`/canvas-plugins/${id}`, { method: 'DELETE' }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ['canvas-plugins'] })
      navigate('/canvas-plugins')
    },
    onError: (error: Error) => showToast(error.message),
  })
  const download = async (version: CanvasPluginVersion) => {
    const blob = await apiRequestBlob(`/canvas-plugins/${id}/versions/${version.id}/download`)
    const url = URL.createObjectURL(blob)
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = `${plugin.data?.packageId.replace('/', '-')}-${version.packageVersion}.agentx-plugin`
    anchor.click()
    URL.revokeObjectURL(url)
  }

  if (plugin.isLoading) return <PageContainer><p className="text-sm text-muted-foreground">{t('common.loading')}</p></PageContainer>
  if (!plugin.data) return <PageContainer><p className="text-sm text-danger">{t('canvasPlugins.loadFailed')}</p></PageContainer>

  const value = plugin.data
  const canManage = auth.hasPermission('canvas_plugin:manage') && value.sourceType === 'imported'
  const actions = canManage ? (
    <Button disabled={uninstall.isPending} onClick={() => {
      if (window.confirm(t('canvasPlugins.confirmUninstall'))) uninstall.mutate()
    }} variant="danger"><Trash2 className="size-4" />{t('canvasPlugins.uninstall')}</Button>
  ) : undefined

  return <PageContainer>
    <PageHeader action={actions} description={value.description} title={value.displayName} />
    <div className="mt-3 flex items-center gap-3 text-xs text-muted-foreground">
      <PackageOpen className="size-4 text-primary" />
      <span className="font-mono">{value.packageId}</span>
      <span>{value.nodeCount} {t('canvasPlugins.fields.nodes')}</span>
    </div>
    <Tabs className="mt-6" defaultValue="versions">
      <TabsList>
        <TabsTrigger value="versions">{t('canvasPlugins.versions')}</TabsTrigger>
        <TabsTrigger value="usage">{t('canvasPlugins.usage')}</TabsTrigger>
        <TabsTrigger value="development">{t('canvasPlugins.development')}</TabsTrigger>
        {canManage && <TabsTrigger value="audit">{t('canvasPlugins.audit')}</TabsTrigger>}
      </TabsList>
      <TabsContent className="mt-4 space-y-3" value="versions">
        {value.versions.map((version) => <Card className="p-4" key={version.id}>
          <div className="flex items-start gap-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <strong className="text-sm">v{version.packageVersion}</strong>
                <StatusBadge status={version.status === 'enabled' ? 'active' : 'inactive'} />
                {value.defaultVersionId === version.id && <span className="rounded bg-primary/10 px-2 py-0.5 text-[10px] text-primary">{t('canvasPlugins.defaultVersion')}</span>}
              </div>
              <p className="mt-2 break-all font-mono text-[9px] text-muted-foreground">{version.bundleDigest}</p>
              <div className="mt-3 flex flex-wrap gap-2">
                {version.nodeTypes.map((node) => <span className="rounded bg-muted px-2 py-1 font-mono text-[10px]" key={node}>{node}</span>)}
              </div>
            </div>
            {canManage && <div className="flex shrink-0 flex-wrap justify-end gap-1">
              <Button aria-label={t('canvasPlugins.downloadVersion')} onClick={() => void download(version).catch((error: Error) => showToast(error.message))} size="icon" variant="ghost"><Download className="size-4" /></Button>
              <Button disabled={toggle.isPending} onClick={() => toggle.mutate(version)} size="sm" variant="secondary"><Power className="size-3.5" />{version.status === 'enabled' ? t('canvasPlugins.disable') : t('canvasPlugins.enable')}</Button>
              {version.status === 'enabled' && value.defaultVersionId !== version.id && <Button disabled={setDefault.isPending} onClick={() => setDefault.mutate(version)} size="sm" variant="secondary"><Star className="size-3.5" />{t('canvasPlugins.setDefault')}</Button>}
              <Button aria-label={t('canvasPlugins.deleteVersion')} disabled={removeVersion.isPending} onClick={() => {
                if (window.confirm(t('canvasPlugins.confirmDelete'))) removeVersion.mutate(version)
              }} size="icon" variant="ghost"><Trash2 className="size-4 text-danger" /></Button>
            </div>}
          </div>
        </Card>)}
      </TabsContent>
      <TabsContent className="mt-4" value="usage">
        <Card className="p-4">
          <h2 className="text-xs font-semibold">{t('canvasPlugins.usage')}</h2>
          <p className="mt-4 text-3xl font-semibold">{references.data?.total ?? 0}</p>
          <p className="mt-1 text-[10px] text-muted-foreground">{t('canvasPlugins.referenceCount')}</p>
          <dl className="mt-4 grid grid-cols-2 gap-3 text-xs">
            <Value label={t('canvasPlugins.drafts')} value={references.data?.drafts ?? 0} />
            <Value label={t('canvasPlugins.versions')} value={references.data?.workflowVersions ?? 0} />
            <Value label={t('canvasPlugins.deployments')} value={references.data?.deployments ?? 0} />
            <Value label={t('canvasPlugins.executionArtifacts')} value={references.data?.executions ?? 0} />
          </dl>
        </Card>
      </TabsContent>
      <TabsContent className="mt-4" value="development">
        <Card className="p-4">
          <h2 className="text-xs font-semibold">{t('canvasPlugins.development')}</h2>
          <p className="mt-2 text-xs leading-5 text-muted-foreground">{value.sourceType === 'builtin' ? t('canvasPlugins.builtinHint') : t('canvasPlugins.templateHint')}</p>
        </Card>
      </TabsContent>
      <TabsContent className="mt-4 space-y-2" value="audit">
        {(audit.data ?? []).map((item) => <Card className="p-3" key={item.id}><div className="flex justify-between gap-3 text-xs"><strong>{item.action}</strong><time className="text-muted-foreground">{formatDateTime(item.createdAt)}</time></div><p className="mt-2 max-w-60 truncate text-[11px] text-muted-foreground" title={item.actorUserId}>{item.actorName ?? t('applications.unknownUser')}</p><pre className="mt-2 overflow-auto rounded bg-muted/40 p-2 text-[10px]">{JSON.stringify(item.detail, null, 2)}</pre></Card>)}
      </TabsContent>
    </Tabs>
  </PageContainer>
}

function Value({ label, value }: { label: string; value: number }) {
  return <div><dt className="text-muted-foreground">{label}</dt><dd className="mt-1 font-semibold">{value}</dd></div>
}
