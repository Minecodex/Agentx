const translations = {
  jsonItems: '{{count}} 项', jsonProperties: '{{count}} 个字段',
  loading: '正在加载 Trace…', unavailable: 'Trace 暂时不可用', delayed: 'Trace 正在同步，当前展示已到达的 Span。', degraded: 'Trace 存在摄取冲突，当前诊断信息可能不完整。',
  search: '搜索 Span、ID 或错误码', filterKind: '筛选 Span 类型', allKinds: '全部类型', errorsOnly: '仅异常', expandAll: '全部展开', zoom: '缩放', loaded: '已加载 {{loaded}} / {{total}}', loadMore: '加载更多', noMatches: '没有匹配的 Span',
  tree: 'Trace 层级瀑布', hierarchy: '调用层级', status: '状态', duration: '耗时', totalDuration: '总耗时', timeline: '时间轴', expand: '展开', collapse: '折叠', selectSpan: '选择一个 Span 查看详情', loadingDetail: '正在加载 Span 详情…',
  overview: '概览', events: '事件', raw: '原始数据', startedAt: '开始时间', tokens: '输入 / 输出 Token', cost: '成本', lifecycleOnly: '这是生命周期 Span，没有业务内容。', diagnosticUnavailable: 'Trace 暂不可用；节点的权威输入输出仍可查看。',
  finalOutput: '工作流最终输出', structuredFinalOutput: '结构化最终输出', showFinalJson: '显示最终 JSON', hideFinalJson: '隐藏最终 JSON', workflowInput: '工作流输入', workflowOutput: '工作流输出',
  authoritativeDataUnavailable: 'Runtime 权威执行数据暂不可用。', loadingExecutionData: '正在加载 Runtime 权威执行数据…', authoritativeInputEmpty: 'Runtime 未记录业务输入。', authoritativeOutputEmpty: 'Runtime 尚未产生业务输出。',
  businessInputAndParameters: '输入与解析参数', upstreamItems: '上游 Item', resolvedParameters: '解析参数', semanticOutput: '语义输出', internalProcess: '内部过程', internalProcessHint: '默认折叠，供诊断使用', syncing: 'Trace 正在同步…', noInternalSpans: '该节点没有内部调用。', noResolvedParameters: '该节点没有解析参数事件。', noDiagnosticContent: '该 Span 没有诊断内容。', noBusinessContent: '没有业务内容。', noNodeRuns: '当前执行没有该节点的运行记录。', pluginRendererUnavailable: '冻结的插件渲染器不可用，标准数据仍可查看。',
  itemCount: '{{count}} 个 Item', itemIndex: 'Item {{index}}', rawItem: '原始 Item / 来源 / Binary', emptyObject: '空对象', downloadArtifact: '查看 / 下载 Artifact', usageValue: '{{input}} input · {{output}} output · {{total}} total', runLabel: 'run {{run}} · iteration {{iteration}}',
  views: { label: 'Trace 视图', nodes: '节点视图', waterfall: '高级瀑布', nodesHint: '默认展示业务节点和语义输入输出', waterfallHint: '展示底层 Span 时间线和完整诊断信息' },
  boundaries: { start: '开始', startDescription: 'Start Boundary · 工作流输入', end: '结束', endDescription: 'End Boundary · 工作流输出' },
  waitApproval: '审批 · {{title}}', waitGeneric: '等待 · {{title}}',
  kinds: { execution: '执行', boundary: '边界', node: '节点', attempt: '尝试', agent_run: '智能体运行', agent_iteration: '智能体迭代', runtime_call: 'Runtime 调用', sandbox: '沙箱', wait: '等待 / 审批', plugin_operation: '插件操作' },
  contentKinds: { workflow_input: '工作流输入', workflow_output: '工作流输出', node_input: '节点输入', node_output: '节点输出', attempt_input: '尝试输入', attempt_output: '尝试输出', resolved_parameters: '解析参数', runtime_request: 'Provider / Runtime 请求', runtime_response: 'Provider / Runtime 响应', agent_input: '智能体输入', agent_output: '智能体输出', iteration_input: '迭代输入', iteration_output: '迭代输出', sandbox_request: '沙箱请求', sandbox_response: '沙箱响应', wait_request: '等待请求', wait_response: '等待响应', conversion_record: '字符串转换记录', plugin_content: '插件业务内容' },
} as const
export default translations
