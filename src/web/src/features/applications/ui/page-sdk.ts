import source from '../../../../../../contracts/sdk/application-page-v1.js?raw'

/** Delivered verbatim: production minification cannot hoist a serialized SDK's dependencies. */
export const PAGE_SDK_SOURCE=source
export function pageSdkBootstrap(bindingId:string,script:string){return script.replace('__AGENTX_PAGE_BINDING__',JSON.stringify(bindingId))}
