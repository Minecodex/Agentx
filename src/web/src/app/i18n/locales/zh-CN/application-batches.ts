export default {
  single:'单次调用', batches:'批次记录', detail:'批次明细', name:'批次', source:'来源', version:'业务版本', progress:'已结束', items:'项目', created:'创建时间',  web:'业务网页', api:'应用 API', all:'全部', status:'状态',
  back:'返回批次记录', stop:'停止后续派发', cancel:'取消未结束项', rerun:'重新执行选中失败项', export:'导出汇总', download:'下载 CSV', select:'选择失败项', selected:'已选 {{count}} 项', input:'输入', output:'输出',   task:'查看调用', approval:'查看审批', lineage:'来源批次',
  stopHint:'尚未受理的项目将取消；已受理项目继续运行。',cancelHint:'停止后续派发，并向可取消的原任务发出取消请求。已产生的外部操作不会自动回滚。',rerunHint:'创建使用当前发布的新批次，重新执行选中且符合条件的失败项。原批次保留。',
  confirm:'确认',empty:'暂无批次记录',emptyItems:'没有符合条件的项目',exporting:'正在生成导出文件',exportFailed:'导出失败',active:'正在派发',accepted:'已受理',running:'处理中',queued:'排队中',waiting:'待处理',waiting_input:'待补充资料',waiting_approval:'待审批',completed:'已完成',failed:'失败',partial_failure:'已结束 · 有失败项',cancelled:'已取消',outcome_unknown:'结果待核实',rejected:'受理失败',stopped:'已停止派发',finished:'派发结束',
} as const
