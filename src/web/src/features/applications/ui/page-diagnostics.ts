import type { PageCheckIssue } from './page-types'

/** Controller prefix has one final newline so generated HTML starts at column one. */
export function pageSourceIssues(issues: PageCheckIssue[], sdk: string, styles = ''): PageCheckIssue[] {
  const offset = (sdk.match(/\r\n|[\r\n]/g)?.length ?? 0) + (styles.match(/\r\n|[\r\n]/g)?.length ?? 0) + 1
  return issues.map((issue) => {
    if (issue.code !== 'PAGE_SCRIPT_ERROR' || !issue.line) return issue
    if (issue.line <= offset) return { ...issue, code: 'PAGE_SDK_SCRIPT_ERROR', message: `Platform SDK: ${issue.message}`, line: undefined, column: undefined }
    return { ...issue, line: issue.line - offset }
  })
}
