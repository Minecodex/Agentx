import { writeFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import ts from 'typescript'

import openapiTS, { astToString } from 'openapi-typescript'

const schemaUrl = new URL('../../../contracts/openapi/platform-api.json', import.meta.url)
const defaultOutputUrl = new URL('../src/shared/api/generated.ts', import.meta.url)
const outputPath = process.argv[2]
  ? resolve(process.argv[2])
  : fileURLToPath(defaultOutputUrl)

const generated = astToString(await openapiTS(schemaUrl))
const componentStart = generated.indexOf('export interface components {')
const componentEnd = generated.indexOf('\nexport type $defs', componentStart)

if (componentStart < 0 || componentEnd < 0) {
  throw new Error('Unable to locate the generated OpenAPI components interface')
}

const header = `/** Generated schema components from contracts/openapi/platform-api.json. Do not edit. */\n`
const components = generated
  .slice(componentStart, componentEnd)
  .replace(/^[ \t]*\/\*\* Format: [^\r\n]*\*\/\r?\n/gm, '')

// Generated components stay below the repository line limit; retain one schema member per line.
const scanner = ts.createScanner(ts.ScriptTarget.Latest, true, ts.LanguageVariant.Standard, components)
let depth = 0
let compact = ''
for (let token = scanner.scan(); token !== ts.SyntaxKind.EndOfFileToken; token = scanner.scan()) {
  compact += scanner.getTokenText() + ' '
  if (token === ts.SyntaxKind.OpenBraceToken) depth += 1
  if (token === ts.SyntaxKind.CloseBraceToken) depth -= 1
  if ((token === ts.SyntaxKind.SemicolonToken && depth <= 2) || (token === ts.SyntaxKind.CloseBraceToken && depth === 0)) compact += '\n'
}
await writeFile(outputPath, `${header}${compact.split("\n").map(line => line.trimEnd()).join("\n").trim()}\n`, 'utf8')
