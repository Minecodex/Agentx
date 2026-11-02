import { useMutation,useQuery,useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { useParams,useSearchParams,Link } from 'react-router-dom'
import { useAuth } from '../../app/providers/auth-provider'
import { apiRequest,jsonBody } from '../../shared/api/client'
import type { components } from '../../shared/api/generated'
import { Button } from '../../shared/ui/button'
import { Card } from '../../shared/ui/card'

type Request=components['schemas']['ApplicationOAuthRequest']
type Continuation=components['schemas']['ApplicationOAuthRedirect']
type Grant=components['schemas']['ApplicationOAuthGrant']
export function OAuthPage(){
  const {kind=''}=useParams(),[search]=useSearchParams(),{t}=useTranslation(),auth=useAuth(),query=useQueryClient()
  const challenge=search.get(kind==='login'?'login_challenge':'consent_challenge')??''
  const path=`/oauth/requests/${encodeURIComponent(kind)}/${encodeURIComponent(challenge)}`
  const request=useQuery({queryKey:['oauth-request',kind,challenge,auth.sessionId],enabled:kind!=='grants'&&Boolean(challenge),queryFn:()=>apiRequest<Request>(path),retry:false})
  const grants=useQuery({queryKey:['oauth-grants',auth.sessionId],enabled:kind==='grants',queryFn:()=>apiRequest<Grant[]>('/oauth/grants')})
  const decide=useMutation({mutationFn:(accept:boolean)=>apiRequest<Continuation>(path,{method:'POST',body:jsonBody({accept})}),onSuccess:result=>window.location.assign(result.redirectTo)})
  const revoke=useMutation({mutationFn:(client:string)=>apiRequest(`/oauth/grants/${client}`,{method:'DELETE'}),onSuccess:()=>query.invalidateQueries({queryKey:['oauth-grants']})})
  return <main className="grid min-h-screen place-items-center bg-canvas p-10"><Card className="w-full max-w-xl space-y-6 p-7"><header className="flex items-center justify-between"><span className="text-lg font-semibold">Agentx</span><span className="text-xs text-muted-foreground">{t('applicationMcp.signedIn')} · {auth.user?.displayName}</span></header>
    <h1 className="text-xl font-semibold">{t(kind==='grants'?'applicationMcp.myGrants':kind==='login'?'applicationMcp.loginTitle':'applicationMcp.consentTitle')}</h1>
    {kind==='grants'?<>{grants.data?.length===0&&<p className="text-sm text-muted-foreground">{t('applicationMcp.grantEmpty')}</p>}{grants.data?.map(grant=><div className="flex items-center gap-4 rounded-lg border border-border p-4" key={grant.clientId}><div className="flex-1 text-sm"><strong>{grant.clientName}</strong><p className="mt-1 text-xs text-muted-foreground">{grant.applicationName}</p></div><Button size="sm" variant="secondary" disabled={revoke.isPending} onClick={()=>revoke.mutate(grant.clientId)}>{t('applicationMcp.revoke')}</Button></div>)}{grants.error&&<p role="alert">{grants.error.message}</p>}{revoke.error&&<p className="text-sm text-danger" role="alert">{revoke.error.message}</p>}</>:<>
    {request.isLoading&&<p>{t('common.loading')}</p>}{request.data&&<><div className="space-y-3 rounded-lg border border-border bg-canvas p-4"><h2 className="font-medium">{request.data.clientName} → {request.data.applicationName}</h2><p className="text-sm leading-6 text-muted-foreground">{t('applicationMcp.consentDetail')}</p><code className="block break-all text-xs text-muted-foreground">{request.data.resource}</code></div><div className="flex justify-end gap-3"><Button variant="secondary" disabled={decide.isPending} onClick={()=>decide.mutate(false)}>{t('applicationMcp.deny')}</Button><Button disabled={decide.isPending} onClick={()=>decide.mutate(true)}>{t(kind==='login'?'applicationMcp.continue':'applicationMcp.allow')}</Button></div></>}{(request.error||decide.error)&&<p className="text-sm text-danger" role="alert">{request.error?.message??decide.error?.message}</p>}
    <Link className="block text-xs text-primary underline" to="/oauth/grants">{t('applicationMcp.myGrants')}</Link></>}
  </Card></main>
}
