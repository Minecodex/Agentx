/** Trusted controller + opaque generated document. No credential or generic network capability enters either frame. */
export const MAX_PAGE_BYTES = 1024 * 1024

const policy = "default-src 'none'; base-uri 'none'; object-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'none'; img-src data: blob:; media-src blob:; font-src 'none'; form-action 'none'; worker-src 'none'"
export const CONTROLLER_POLICY = `${policy}; frame-src about:`
export const PAGE_POLICY = `${policy}; frame-src 'none'`

function literal(value: unknown) { return JSON.stringify(value).replaceAll('<', '\\u003c').replaceAll('\u2028', '\\u2028').replaceAll('\u2029', '\\u2029') }

export function createPageController(html: string, bindingId: string, sdk = '', styles = ''): string {
  if (new TextEncoder().encode(html).byteLength > MAX_PAGE_BYTES) throw new Error('PAGE_TOO_LARGE')
  if (!/^[a-zA-Z0-9_-]{16,80}$/.test(bindingId)) throw new Error('PAGE_BINDING_INVALID')
  const inner = `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${PAGE_POLICY}"><meta name="referrer" content="no-referrer"><style>${styles}</style><script>${sdk}</script></head><body>\n${html}</body></html>`
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${CONTROLLER_POLICY}"><meta name="referrer" content="no-referrer"><style>html,body,iframe{margin:0;width:100%;height:100%;border:0;display:block}</style></head><body><script>
    'use strict';
    const bindingId=${literal(bindingId)};
    const frame=document.createElement('iframe');frame.sandbox='allow-scripts allow-forms';frame.referrerPolicy='no-referrer';
    frame.title='Application page';frame.name='agentx-application-page';
    let transferred=false,ready=false,loads=0;
    function revoke(){ready=false;parent.postMessage({type:'agentx.ui.revoked',bindingId},'*');}
    frame.addEventListener('load',()=>{if(++loads>1)revoke();});
    addEventListener('message',event=>{
      if(event.source===frame.contentWindow&&event.data?.type==='agentx.page.ready'&&event.data.bindingId===bindingId&&!ready){
        ready=true;parent.postMessage({type:'agentx.ui.ready',bindingId},'*');
      }else if(event.source===parent&&event.data?.type==='agentx.ui.bind'&&event.data.bindingId===bindingId&&ready&&!transferred&&event.ports.length===1){
        transferred=true;frame.contentWindow.postMessage({type:'agentx.page.bind',bindingId},'*',[event.ports[0]]);
      }
    });
    frame.srcdoc=${literal(inner)};document.body.append(frame);
    addEventListener('pagehide',revoke,{once:true});
  </script></body></html>`
}
