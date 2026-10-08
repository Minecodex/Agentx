import { RefreshCw, TriangleAlert } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { PageContainer } from '../shared/components/page-container'
import { Button } from '../shared/ui/button'

export function RouteErrorPage() {
  const { t } = useTranslation()
  return (
    <PageContainer className="grid min-h-screen place-items-center">
      <div className="max-w-md text-center" role="alert">
        <span className="mx-auto grid size-16 place-items-center rounded-2xl bg-primary/10 text-primary"><TriangleAlert className="size-7" /></span>
        <h1 className="mt-5 text-2xl font-bold">{t('errors.pageLoad.title')}</h1>
        <p className="mt-2 text-sm text-muted-foreground">{t('errors.pageLoad.description')}</p>
        <div className="mt-6 flex justify-center gap-3">
          <Button onClick={() => window.location.reload()}><RefreshCw className="size-4" />{t('errors.pageLoad.reload')}</Button>
          <Button asChild variant="secondary"><a href="/">{t('errors.pageLoad.home')}</a></Button>
        </div>
      </div>
    </PageContainer>
  )
}
