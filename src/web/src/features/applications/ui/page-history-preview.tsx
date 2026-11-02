import { useQuery } from '@tanstack/react-query'
import { useEffect, useMemo } from 'react'
import { useTranslation } from 'react-i18next'

import { apiRequest } from '../../../shared/api/client'
import { PageFrame } from './page-frame'
import { createPagePreview } from './page-preview'
import type { PublishedPage } from './page-types'

export function PageHistoryPreview({applicationId,publicationId}:{applicationId:string;publicationId:string}) {
  const {t}=useTranslation()
  const page=useQuery({queryKey:['application-page-history-preview',applicationId,publicationId],queryFn:()=>apiRequest<PublishedPage>(`/applications/${applicationId}/ui/publications/${publicationId}/preview`)})
  if(page.error)return <p role="alert" className="p-5 text-sm text-danger">{page.error.message}</p>
  return page.data?<FrozenPreview key={publicationId} page={page.data}/>:<p role="status" className="p-5 text-sm text-muted-foreground">{t('applicationUi.loading')}</p>
}

function FrozenPreview({page}:{page:PublishedPage}) {
  const {i18n}=useTranslation()
  const preview=useMemo(()=>createPagePreview(page.publication.contract,page.publication.contractHash,i18n.language,document.documentElement.classList.contains('dark')?'dark':'light'),[page,i18n.language])
  useEffect(()=>()=>preview.close(),[preview])
  return <div className="h-full" data-testid="application-page-history-preview"><PageFrame fit html={page.html} sdkScript={page.sdkScript} stylesheet={page.stylesheet} identity={`history:${page.publication.publicationId}`} dispatch={preview.dispatch} onDisconnect={preview.close}/></div>
}
