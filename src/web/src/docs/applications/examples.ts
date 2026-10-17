type JsonSchema = {
  default?: unknown
  enum?: unknown[]
  examples?: unknown[]
  properties?: Record<string, JsonSchema>
  items?: JsonSchema
  type?: string | string[]
}

export const integrationLanguages = ['curl', 'java', 'go', 'node', 'python'] as const
export type IntegrationLanguage = typeof integrationLanguages[number]
export type ApiKeyEndpointId = 'createInvocation' | 'createSession' | 'sendMessage' | 'getInvocation' | 'streamEvents' | 'cancelInvocation'

type RequestExample = {
  body?: unknown
  headers?: Array<[string, string]>
  method: 'GET' | 'POST'
  stream?: boolean
  url: string
}

export function schemaExample(schema: unknown): unknown {
  return exampleValue(isSchema(schema) ? schema : {}, 0)
}

export function apiKeyExample(endpointId: ApiKeyEndpointId, language: IntegrationLanguage, runtimeBaseUrl: string, slug: string, inputSchema: unknown): string {
  return renderRequest(apiKeyRequest(endpointId, runtimeBaseUrl, slug, inputSchema), language, 'AGENTX_API_KEY')
}

export function webhookExample(language: IntegrationLanguage, runtimeBaseUrl: string, path: string | undefined, inputSchema: unknown): string {
  const url = path ? webhookUrl(runtimeBaseUrl, path) : `${gatewayBaseUrl(runtimeBaseUrl)}/webhooks/{publicId}`
  const body = JSON.stringify(schemaExample(inputSchema))
  if (language === 'curl') return webhookCurl(url, body)
  if (language === 'java') return webhookJava(url, body)
  if (language === 'go') return webhookGo(url, body)
  if (language === 'python') return webhookPython(url, body)
  return webhookNode(url, body)
}

export function applicationInvocationUrl(runtimeBaseUrl: string, slug: string): string {
  return `${gatewayBaseUrl(runtimeBaseUrl)}/applications/${encodeURIComponent(slug)}/invocations`
}

export function webhookUrl(runtimeBaseUrl: string, path: string): string {
  const base = trimTrailingSlash(runtimeBaseUrl)
  const normalizedPath = path.startsWith('/') ? path : `/${path}`
  return `${base}${normalizedPath}`
}

function apiKeyRequest(endpointId: ApiKeyEndpointId, runtimeBaseUrl: string, slug: string, inputSchema: unknown): RequestExample {
  const base = gatewayBaseUrl(runtimeBaseUrl)
  const auth: [string, string] = ['Authorization', 'Bearer {apiKey}']
  const idempotency: [string, string] = ['Idempotency-Key', `${endpointId}-10001`]
  if (endpointId === 'createInvocation') return {
    method: 'POST',
    url: applicationInvocationUrl(runtimeBaseUrl, slug),
    headers: [auth, idempotency, ['Content-Type', 'application/json']],
    body: { input: schemaExample(inputSchema), responseMode: 'async' },
  }
  if (endpointId === 'createSession') return {
    method: 'POST',
    url: `${base}/applications/${encodeURIComponent(slug)}/sessions`,
    headers: [auth, idempotency, ['Content-Type', 'application/json']],
    body: { title: 'Support conversation', input: { customerId: 'customer-10001' } },
  }
  if (endpointId === 'sendMessage') return {
    method: 'POST',
    url: `${base}/sessions/{sessionId}/messages`,
    headers: [auth, idempotency, ['Content-Type', 'application/json']],
    body: { parts: [{ partType: 'text', content: 'Hello Agentx' }] },
  }
  if (endpointId === 'getInvocation') return { method: 'GET', url: `${base}/invocations/{invocationId}`, headers: [auth] }
  if (endpointId === 'streamEvents') return { method: 'GET', url: `${base}/invocations/{invocationId}/events`, headers: [auth, ['Accept', 'text/event-stream'], ['Last-Event-ID', '0']], stream: true }
  return { method: 'POST', url: `${base}/invocations/{invocationId}/cancel`, headers: [auth, idempotency] }
}

function renderRequest(request: RequestExample, language: IntegrationLanguage, secretName: string): string {
  if (language === 'curl') return curlRequest(request, secretName)
  if (language === 'java') return javaRequest(request, secretName)
  if (language === 'go') return goRequest(request, secretName)
  if (language === 'python') return pythonRequest(request, secretName)
  return nodeRequest(request, secretName)
}

function curlRequest(request: RequestExample, secretName: string) {
  const url = request.url.replace('{sessionId}', '$SESSION_ID').replace('{invocationId}', '$INVOCATION_ID')
  const lines = [`curl --request ${request.method} "${url}"`]
  for (const [name, rawValue] of request.headers ?? []) {
    const value = rawValue.replace('{apiKey}', `$${secretName}`)
    lines.push(`  --header ${value.includes('$') ? `"${name}: ${value}"` : `'${name}: ${value}'`}`)
  }
  if (request.body !== undefined) lines.push(`  --data '${escapeShellSingleQuoted(JSON.stringify(request.body, null, 2))}'`)
  return lines.join(' \\\n')
}

function javaRequest(request: RequestExample, secretName: string) {
  const body = request.body === undefined ? '' : `\n    var body = ${JSON.stringify(JSON.stringify(request.body))};`
  const url = javaUrl(request.url)
  const builder = request.method === 'POST'
    ? `.POST(${request.body === undefined ? 'HttpRequest.BodyPublishers.noBody()' : 'HttpRequest.BodyPublishers.ofString(body)'})`
    : '.GET()'
  const headers = (request.headers ?? []).map(([name, value]) => `      .header("${name}", ${javaHeaderValue(value, secretName)})`).join('\n')
  const handler = request.stream ? 'HttpResponse.BodyHandlers.ofLines()' : 'HttpResponse.BodyHandlers.ofString()'
  const output = request.stream ? '    response.body().forEach(System.out::println);' : '    System.out.println(response.statusCode() + " " + response.body());'
  return `import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

public class AgentxExample {
  public static void main(String[] args) throws Exception {${body}
    var request = HttpRequest.newBuilder(URI.create(${url}))
${headers}
      ${builder}
      .build();
    var response = HttpClient.newHttpClient().send(request, ${handler});
${output}
  }
}`
}

function goRequest(request: RequestExample, secretName: string) {
  const body = request.body === undefined ? 'nil' : `strings.NewReader(${JSON.stringify(JSON.stringify(request.body))})`
  const headers = (request.headers ?? []).map(([name, value]) => `req.Header.Set(${JSON.stringify(name)}, ${goHeaderValue(value, secretName)})`).join('\n')
  const stringsImport = request.body === undefined ? '' : '\n  "strings"'
  return `package main

import (
  "fmt"
  "io"
  "net/http"
  "os"${stringsImport}
)

func main() {
  url := ${goUrl(request.url)}
  req, err := http.NewRequest(${JSON.stringify(request.method)}, url, ${body})
  if err != nil { panic(err) }
  ${headers}
  response, err := http.DefaultClient.Do(req)
  if err != nil { panic(err) }
  defer response.Body.Close()
  payload, err := io.ReadAll(response.Body)
  if err != nil { panic(err) }
  fmt.Println(response.StatusCode, string(payload))
}`
}

function nodeRequest(request: RequestExample, secretName: string) {
  const url = nodeUrl(request.url)
  const headers = nodeHeaders(request.headers ?? [], secretName)
  const body = request.body === undefined ? '' : `,\n  body: JSON.stringify(${JSON.stringify(request.body, null, 2)})`
  if (request.stream) return `const response = await fetch(${url}, {
  headers: ${headers},
})

if (!response.ok) throw new Error(\`HTTP \${response.status}: \${await response.text()}\`)
const reader = response.body.pipeThrough(new TextDecoderStream()).getReader()
while (true) {
  const { value, done } = await reader.read()
  if (done) break
  process.stdout.write(value)
}`
  return `const response = await fetch(${url}, {
  method: '${request.method}',
  headers: ${headers}${body},
})

console.log(response.status, await response.json())`
}

function pythonRequest(request: RequestExample, secretName: string) {
  const bodyLine = request.body === undefined ? '' : `payload = json.dumps(${pythonLiteral(request.body)}).encode()\n`
  const data = request.body === undefined ? '' : ', data=payload'
  const headers = Object.fromEntries((request.headers ?? []).map(([name, value]) => [name, value.replace('{apiKey}', `' + os.environ['${secretName}']`)]))
  const headerLiteral = JSON.stringify(headers, null, 2).replace(/"Bearer ' \+ os\.environ\['([^']+)'\]"/g, `"Bearer " + os.environ['$1']`)
  const output = request.stream ? '    for line in response:\n        print(line.decode().rstrip())' : '    print(response.status, json.load(response))'
  return `import json
import os
import urllib.request

${bodyLine}url = ${pythonUrl(request.url)}
request = urllib.request.Request(
    url,
    method='${request.method}',
    headers=${indent(headerLiteral, 4)}${data},
)
with urllib.request.urlopen(request) as response:
${output}`
}

function webhookCurl(url: string, body: string) {
  return `BODY='${escapeShellSingleQuoted(body)}'
TIMESTAMP=$(date +%s)
SIGNATURE=$(printf '%s.%s' "$TIMESTAMP" "$BODY" \\
  | openssl dgst -sha256 -mac HMAC -macopt "key:$AGENTX_WEBHOOK_SECRET" -binary \\
  | openssl base64 -A | tr '+/' '-_' | tr -d '=')

curl --request POST "${url.replace('{publicId}', '$WEBHOOK_PUBLIC_ID')}" \\
  --header 'Content-Type: application/json' \\
  --header 'Idempotency-Key: webhook-10001' \\
  --header "X-Agentx-Timestamp: $TIMESTAMP" \\
  --header "X-Agentx-Signature: $SIGNATURE" \\
  --data "$BODY"`
}

function webhookJava(url: string, body: string) {
  return `import java.net.URI;
import java.net.http.*;
import java.nio.charset.StandardCharsets;
import java.time.Instant;
import java.util.Base64;
import javax.crypto.Mac;
import javax.crypto.spec.SecretKeySpec;

public class AgentxWebhookExample {
  public static void main(String[] args) throws Exception {
    var body = ${JSON.stringify(body)};
    var timestamp = Long.toString(Instant.now().getEpochSecond());
    var mac = Mac.getInstance("HmacSHA256");
    mac.init(new SecretKeySpec(System.getenv("AGENTX_WEBHOOK_SECRET").getBytes(StandardCharsets.UTF_8), "HmacSHA256"));
    var signature = Base64.getUrlEncoder().withoutPadding().encodeToString(mac.doFinal((timestamp + "." + body).getBytes(StandardCharsets.UTF_8)));
    var url = ${javaUrl(url)};
    var request = HttpRequest.newBuilder(URI.create(url))
      .header("Content-Type", "application/json")
      .header("Idempotency-Key", "webhook-10001")
      .header("X-Agentx-Timestamp", timestamp)
      .header("X-Agentx-Signature", signature)
      .POST(HttpRequest.BodyPublishers.ofString(body)).build();
    var response = HttpClient.newHttpClient().send(request, HttpResponse.BodyHandlers.ofString());
    System.out.println(response.statusCode() + " " + response.body());
  }
}`
}

function webhookGo(url: string, body: string) {
  return `package main

import (
  "crypto/hmac"
  "crypto/sha256"
  "encoding/base64"
  "fmt"
  "io"
  "net/http"
  "os"
  "strconv"
  "strings"
  "time"
)

func main() {
  body := ${JSON.stringify(body)}
  timestamp := strconv.FormatInt(time.Now().Unix(), 10)
  mac := hmac.New(sha256.New, []byte(os.Getenv("AGENTX_WEBHOOK_SECRET")))
  mac.Write([]byte(timestamp + "." + body))
  signature := base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
  url := ${goUrl(url)}
  req, _ := http.NewRequest("POST", url, strings.NewReader(body))
  req.Header.Set("Content-Type", "application/json")
  req.Header.Set("Idempotency-Key", "webhook-10001")
  req.Header.Set("X-Agentx-Timestamp", timestamp)
  req.Header.Set("X-Agentx-Signature", signature)
  response, err := http.DefaultClient.Do(req)
  if err != nil { panic(err) }
  defer response.Body.Close()
  payload, _ := io.ReadAll(response.Body)
  fmt.Println(response.StatusCode, string(payload))
}`
}

function webhookNode(url: string, body: string) {
  return `import { createHmac } from 'node:crypto'

const body = ${JSON.stringify(body)}
const timestamp = Math.floor(Date.now() / 1000).toString()
const signature = createHmac('sha256', process.env.AGENTX_WEBHOOK_SECRET)
  .update(\`\${timestamp}.\${body}\`)
  .digest('base64url')
const response = await fetch(${nodeUrl(url)}, {
  method: 'POST',
  headers: {
    'Content-Type': 'application/json',
    'Idempotency-Key': 'webhook-10001',
    'X-Agentx-Timestamp': timestamp,
    'X-Agentx-Signature': signature,
  },
  body,
})
console.log(response.status, await response.json())`
}

function webhookPython(url: string, body: string) {
  return `import base64
import hashlib
import hmac
import json
import os
import time
import urllib.request

body = ${JSON.stringify(body)}.encode()
timestamp = str(int(time.time()))
digest = hmac.new(os.environ['AGENTX_WEBHOOK_SECRET'].encode(), timestamp.encode() + b'.' + body, hashlib.sha256).digest()
signature = base64.urlsafe_b64encode(digest).rstrip(b'=').decode()
request = urllib.request.Request(
    ${pythonUrl(url)}, data=body, method='POST',
    headers={
        'Content-Type': 'application/json',
        'Idempotency-Key': 'webhook-10001',
        'X-Agentx-Timestamp': timestamp,
        'X-Agentx-Signature': signature,
    },
)
with urllib.request.urlopen(request) as response:
    print(response.status, json.load(response))`
}

function javaUrl(url: string) {
  return JSON.stringify(url)
    .replace('{sessionId}', `" + System.getenv("SESSION_ID") + "`)
    .replace('{invocationId}', `" + System.getenv("INVOCATION_ID") + "`)
    .replace('{publicId}', `" + System.getenv("WEBHOOK_PUBLIC_ID") + "`)
}

function goUrl(url: string) {
  return JSON.stringify(url)
    .replace('{sessionId}', `" + os.Getenv("SESSION_ID") + "`)
    .replace('{invocationId}', `" + os.Getenv("INVOCATION_ID") + "`)
    .replace('{publicId}', `" + os.Getenv("WEBHOOK_PUBLIC_ID") + "`)
}

function nodeUrl(url: string) {
  const value = url
    .replace('{sessionId}', '${process.env.SESSION_ID}')
    .replace('{invocationId}', '${process.env.INVOCATION_ID}')
    .replace('{publicId}', '${process.env.WEBHOOK_PUBLIC_ID}')
  return `\`${value}\``
}

function pythonUrl(url: string) {
  return `f${JSON.stringify(url)
    .replace('{sessionId}', `{os.environ['SESSION_ID']}`)
    .replace('{invocationId}', `{os.environ['INVOCATION_ID']}`)
    .replace('{publicId}', `{os.environ['WEBHOOK_PUBLIC_ID']}`)}`
}

function javaHeaderValue(value: string, secretName: string) {
  if (value === 'Bearer {apiKey}') return `"Bearer " + System.getenv("${secretName}")`
  return JSON.stringify(value)
}

function goHeaderValue(value: string, secretName: string) {
  if (value === 'Bearer {apiKey}') return `"Bearer " + os.Getenv("${secretName}")`
  return JSON.stringify(value)
}

function nodeHeaders(headers: Array<[string, string]>, secretName: string) {
  const rows = headers.map(([name, value]) => {
    const renderedValue = value === 'Bearer {apiKey}' ? `'Bearer ' + process.env.${secretName}` : JSON.stringify(value)
    return `  ${JSON.stringify(name)}: ${renderedValue}`
  })
  return `{\n${rows.join(',\n')}\n}`
}

function gatewayBaseUrl(runtimeBaseUrl: string): string {
  return `${trimTrailingSlash(runtimeBaseUrl)}/gateway/v1`
}

function trimTrailingSlash(value: string): string {
  return value.replace(/\/$/, '')
}

function escapeShellSingleQuoted(value: string): string {
  return value.replaceAll("'", "'\"'\"'")
}

function indent(value: string, spaces: number) {
  return value.replace(/\n/g, `\n${' '.repeat(spaces)}`)
}

function pythonLiteral(value: unknown): string {
  return JSON.stringify(value, null, 2).replace(/\btrue\b/g, 'True').replace(/\bfalse\b/g, 'False').replace(/\bnull\b/g, 'None')
}

function exampleValue(schema: JsonSchema, depth: number): unknown {
  if (depth > 4) return null
  if (schema.examples?.length) return schema.examples[0]
  if (schema.default !== undefined) return schema.default
  if (schema.enum?.length) return schema.enum[0]
  const types = Array.isArray(schema.type) ? schema.type : schema.type ? [schema.type] : []
  if (schema.properties || types.includes('object')) {
    return Object.fromEntries(Object.entries(schema.properties ?? {}).map(([name, child]) => [name, exampleValue(child, depth + 1)]))
  }
  if (schema.items || types.includes('array')) return [exampleValue(schema.items ?? {}, depth + 1)]
  if (types.includes('boolean')) return true
  if (types.includes('integer') || types.includes('number')) return 0
  if (types.includes('string')) return 'example'
  return depth === 0 ? {} : null
}

function isSchema(value: unknown): value is JsonSchema {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}
