import { useQuery } from '@tanstack/react-query'
import { MessageSquare, SlidersHorizontal } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useSearchParams } from 'react-router-dom'

import { apiRequest, gatewayRequest } from '../../shared/api/client'
import type { Application, PageResponse } from '../../shared/api/types'
import type { ArtifactReference } from '../../shared/components/schema-form'
import { PageContainer } from '../../shared/components/page-container'
import { PageHeader } from '../../shared/components/page-header'
import { Card } from '../../shared/ui/card'
import { Select } from '../../shared/ui/select'
import { Tabs, TabsList, TabsTrigger } from '../../shared/ui/tabs'
import { ConversationTestWorkspace } from './conversation-test-workspace'
import { ParameterTestWorkspace } from './parameter-test-workspace'
import type { PlaygroundDeployment, UploadedArtifact } from './playground-types'
import { artifactPartType } from './playground-types'

type TestMode = 'parameters' | 'conversation'

export function PlaygroundPage() {
  const { t } = useTranslation()
  const [searchParams, setSearchParams] = useSearchParams()
  const applicationId = searchParams.get('applicationId') ?? ''
  const sessionId = searchParams.get('sessionId') ?? ''
  const mode = searchParams.get('mode') === 'conversation' ? 'conversation' : 'parameters'
  const applications = useQuery({ queryKey: ['applications', 'playground'], queryFn: () => apiRequest<PageResponse<Application>>('/applications?pageSize=100&status=active') })
  const application = applications.data?.items.find((item) => item.id === applicationId)
  const deployments = useQuery({ queryKey: ['application-deployments', applicationId], queryFn: () => apiRequest<PlaygroundDeployment[]>(`/applications/${applicationId}/deployments`), enabled: Boolean(applicationId) })
  const deployment = deployments.data?.find((item) => item.id === application?.activeDeploymentId && item.status === 'active')
  const updateLocation = (patch: { applicationId?: string; sessionId?: string; mode?: TestMode }) => {
    const nextApplication = patch.applicationId ?? applicationId
    const nextMode = patch.mode ?? mode
    const nextSession = patch.sessionId ?? (patch.applicationId !== undefined || nextMode !== 'conversation' ? '' : sessionId)
    const next = new URLSearchParams()
    if (nextApplication) next.set('applicationId', nextApplication)
    next.set('mode', nextMode)
    if (nextMode === 'conversation' && nextSession) next.set('sessionId', nextSession)
    setSearchParams(next)
  }
  const uploadArtifact = async (file: File): Promise<ArtifactReference> => {
    const body = new FormData()
    body.append('file', file)
    const uploaded = await gatewayRequest<UploadedArtifact>('/artifacts', { method: 'POST', headers: { 'Idempotency-Key': crypto.randomUUID() }, body })
    return { artifactId: uploaded.artifactId, fileName: file.name, contentType: uploaded.contentType, sizeBytes: uploaded.sizeBytes, sha256: uploaded.sha256, type: artifactPartType(uploaded.contentType) }
  }
  return <PageContainer className="flex h-full min-h-0 flex-col overflow-hidden">
    <PageHeader action={<Select className="w-72" onValueChange={(value) => updateLocation({ applicationId: value })} options={(applications.data?.items ?? []).map((item) => ({ value: item.id, label: item.name }))} placeholder={t('applications.playground.selectApp')} value={applicationId} />} description={t('applications.playground.description')} title={t('applications.playground.title')} />
    <Tabs className="mt-4" onValueChange={(value) => updateLocation({ mode: value as TestMode })} value={mode}><TabsList><TabsTrigger value="parameters"><SlidersHorizontal className="mr-2 size-4" />{t('applications.playground.parametersMode')}</TabsTrigger><TabsTrigger value="conversation"><MessageSquare className="mr-2 size-4" />{t('applications.playground.conversationMode')}</TabsTrigger></TabsList></Tabs>
    <div className="mt-4 min-h-0 flex-1">{!application ? <EmptyState title={t('applications.playground.selectApplicationTitle')} description={t('applications.playground.selectApplicationDescription')} /> : deployments.isLoading ? <EmptyState title={t('applications.playground.loadingDeployment')} description={t('applications.playground.loadingHint')} /> : !deployment ? <EmptyState title={t('applications.playground.noDeployment')} description={t('applications.playground.noDeploymentDescription')} /> : mode === 'parameters' ? <ParameterTestWorkspace application={application} deployment={deployment} uploadArtifact={uploadArtifact} /> : <ConversationTestWorkspace application={application} deployment={deployment} onSessionChange={(value) => updateLocation({ sessionId: value, mode: 'conversation' })} sessionId={sessionId} uploadArtifact={uploadArtifact} />}</div>
  </PageContainer>
}

function EmptyState({ title, description }: { title: string; description: string }) { return <Card className="grid h-full min-h-0 place-items-center"><div className="max-w-md text-center"><strong className="text-sm">{title}</strong><p className="mt-2 text-xs text-muted-foreground">{description}</p></div></Card> }
