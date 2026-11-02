import source from '../../../../../../contracts/sdk/application-page-v1.css?raw'

/** The stylesheet is immutable; the trusted host supplies the current platform theme tokens. */
export const PAGE_STYLE_SOURCE=source
export function pageStyles(stylesheet:string) {
  const computed=getComputedStyle(document.documentElement)
  const names=['background','foreground','surface','muted','muted-foreground','border','primary','primary-foreground','success','warning','danger','font-sans']
  const tokens=names.map(name=>[name,computed.getPropertyValue(name==='font-sans'?'--font-sans':`--ui-${name}`).trim()]).filter(([,value])=>value&&/^[^;{}<>]+$/.test(value!)).map(([name,value])=>`--${name}:${value}`).join(';')
  return `:root{${tokens}}${stylesheet}`
}
