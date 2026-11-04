import { useSearchParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../../shared/ui/tabs'
import { ApplicationInvocations } from '../application-invocations'
import { ApplicationBatches } from './application-batches'

export function ApplicationCallRecords({applicationId}:{applicationId:string}){
  const {t}=useTranslation(),[params,setParams]=useSearchParams()
  const value=params.get('recordTab')==='batch'?'batch':'single'
  return <Tabs value={value} onValueChange={next=>setParams(current=>{const p=new URLSearchParams(current);p.set('recordTab',next);p.delete('batchId');return p})}>
    <TabsList aria-label={t('applications.invocationRecords')}><TabsTrigger value="single">{t('applicationBatches.single')}</TabsTrigger><TabsTrigger value="batch">{t('applicationBatches.batches')}</TabsTrigger></TabsList>
    <TabsContent value="single" className="pt-4"><ApplicationInvocations applicationId={applicationId}/></TabsContent>
    <TabsContent value="batch" className="pt-4"><ApplicationBatches applicationId={applicationId}/></TabsContent>
  </Tabs>
}
