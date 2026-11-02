"""A real asynchronous Sandbox computation, intermediate messages and report Artifact."""

from tests.e2e.product.plan9.file_scenarios import OUTPUT_FILE, report_node
from tests.e2e.product.plan9.scenarios import reference
from tests.e2e.product.plan9.workflow_builders import code, literal, node, object_schema, output_binding, publish


def long_report(control, profile, run_id, endpoint):
    schema = object_schema(
        {
            "request_id": {"type": "string", "title": "报告编号"},
            "description": {"type": "string", "title": "报告主题和说明", "minLength": 5},
        }
    )
    cases = [
        {"request_id": "S20-REPORT-482", "description": "华东市场第四季度客户回访汇总"},
        {"request_id": "S20-REPORT-719", "description": "工业设备售后保养和备件需求分析"},
    ]
    expected = [
        {"request_id": case["request_id"], "summary": f"异步报告 {case['request_id']}：{case['description']}"}
        for case in cases
    ]
    compute = code(
        "analyze",
        "汇总长报告数据",
        profile,
        {key: reference("inputs", [key]) for key in ("request_id", "description")},
        "def main(**inputs):\n    import time\n    time.sleep(10)\n    return {'request_id': inputs['request_id'], 'summary': '异步报告 ' + inputs['request_id'] + '：' + inputs['description']}",
        {"request_id": "", "summary": ""},
    )
    return publish(
        control,
        run_id,
        "S20",
        "异步长报告",
        schema,
        {"request_id": {"type": "string"}, "summary": {"type": "string"}, "files": OUTPUT_FILE},
        [
            node("progress", "reply_message", "通知已开始汇总", {"content": literal("已进入报告队列，正在汇总数据")}),
            compute,
            node("delivery", "reply_message", "通知报告正在交付", {"content": literal("数据汇总完成，正在整理报告")}),
            report_node(endpoint),
        ],
        {
            "request_id": output_binding("analyze", "request_id"),
            "summary": output_binding("analyze", "summary"),
            "files": reference("outputs", ["files"], "report"),
        },
        cases,
        expected,
        context={
            "fileOutputs": True,
            "downloadText": [f"Agentx business report\n{result['summary']}\n" for result in expected],
            "cancellation": True,
            "intermediateMessages": ["已进入报告队列，正在汇总数据", "数据汇总完成，正在整理报告"],
        },
    )
