import type { Locator, Page } from '@playwright/test'

/** Attach both handlers immediately so a failed action remains a repairable validation error. */
export async function clickForResponse(page:Page,action:Locator,predicate:Parameters<Page['waitForResponse']>[0]){
  await action.waitFor({state:'visible'})
  const [response]=await Promise.all([page.waitForResponse(predicate),action.click()])
  return response
}

/** Evidence reads follow the credential refreshed by the actual browser session. */
export function trackAccessToken(page:Page,initial:string){
  let current=Promise.resolve(initial)
  page.on('response',response=>{
    if(!response.url().endsWith('/api/v1/auth/refresh')||response.request().method()!=='POST'||!response.ok())return
    current=response.json().then((body:{accessToken?:unknown})=>{
      if(typeof body.accessToken!=='string'||!body.accessToken)throw new Error('The browser refresh did not return an access token')
      return body.accessToken
    })
    void current.catch(()=>{})
  })
  return()=>current
}
