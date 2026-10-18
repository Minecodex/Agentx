import { useTranslation } from 'react-i18next'
import { useSearchParams } from 'react-router-dom'
import { useAuth } from '../../../app/providers/auth-provider'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../../shared/ui/tabs'
import { ApplicationDatasets } from './datasets'
import { EvaluationProfiles } from './profiles'
import { ApplicationEvaluations } from './evaluations'

export function ApplicationTestingPanel({ applicationId, activeVersionNumber }: { applicationId: string; activeVersionNumber?: number | null }) {
  const { t } = useTranslation()
  const auth = useAuth()
  const [params, setParams] = useSearchParams()
  const available = [auth.hasPermission('dataset:view') && 'datasets', auth.hasPermission('evaluation:view') && 'runs', auth.hasPermission('evaluation_profile:view') && 'profiles'].filter(Boolean) as string[]
  const tab = available.includes(params.get('testTab') ?? '') ? params.get('testTab')! : available[0]
  return <Tabs value={tab} onValueChange={value => setParams(current => { const next = new URLSearchParams(current); next.set('tab', 'testing'); next.set('testTab', value); next.delete('datasetId'); next.delete('batchId'); return next })}>
    <TabsList className="mb-5 border-b border-border">{available.map(value => <TabsTrigger key={value} value={value}>{t(`testing.${value}`)}</TabsTrigger>)}</TabsList>
    <TabsContent value="datasets"><ApplicationDatasets applicationId={applicationId} activeVersionNumber={activeVersionNumber} /></TabsContent>
    <TabsContent value="runs"><ApplicationEvaluations applicationId={applicationId} /></TabsContent>
    <TabsContent value="profiles"><EvaluationProfiles applicationId={applicationId} /></TabsContent>
  </Tabs>
}
