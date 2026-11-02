import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'

import { apiRequest, jsonBody } from '../../../shared/api/client'
import type { ApplicationDeployment } from '../../../shared/api/types'
import { PageEditor } from './page-editor'
import type { PageDraft } from './page-types'

export function ApplicationInterfaceTab({applicationId,deployments}:{applicationId:string;deployments:ApplicationDeployment[]}) {
  const {t}=useTranslation()
  const deployment=deployments.find(value=>value.status==='active')??deployments.find(value=>value.status==='superseded')
  const page=useQuery({queryKey:['application-ui-editor',applicationId],enabled:Boolean(deployment),queryFn:async()=>{
    const existing=await apiRequest<PageDraft[]>(`/applications/${applicationId}/ui/drafts`)
    if(existing[0])return existing[0]
    if(!deployment)return null
    return apiRequest<PageDraft>(`/applications/${applicationId}/ui/drafts`,{method:'POST',body:jsonBody({deploymentId:deployment.id})})
  },retry:false})
  return <div data-testid="application-interface-tab">
    {page.data?<PageEditor key={page.data.id} draft={page.data}/>:<div className="rounded-lg border border-border bg-surface px-6 py-20 text-center text-sm text-muted-foreground" role={page.error?'alert':'status'}>
      {page.error?.message??t(page.isFetching?'applicationUi.loadingEditor':'applicationUi.empty')}
    </div>}
  </div>
}
