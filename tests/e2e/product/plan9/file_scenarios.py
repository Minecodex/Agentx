"""File-integrity analysis and comparison with real Artifact uploads and report downloads."""

import hashlib

from tests.e2e.product.plan9.scenarios import reference
from tests.e2e.product.plan9.workflow_builders import code, literal, node, object_schema, output_binding, publish

ARTIFACT = object_schema(
    {
        "artifactId": {"type": "string", "format": "uuid"},
        "contentType": {"type": "string"},
        "sizeBytes": {"type": "integer", "minimum": 0},
        "sha256": {"type": "string"},
    }
)
ARTIFACT["x-agentx-artifact"] = True
OUTPUT_FILE = {
    "type": "array",
    "items": object_schema({**ARTIFACT["properties"], "fileName": {"type": "string", "minLength": 1}}),
    "x-agentx-artifact-array": True,
}


def file(name, content):
    return {"__file": {"name": name, "mimeType": "text/plain", "content": content}}


def report_node(endpoint, source="analyze"):
    return node(
        "report",
        "declarative_http",
        "交付正式业务报告文件",
        {
            "method": "GET",
            "url": literal(f"{endpoint}/v1/plan9/report"),
            "headers": [],
            "query": [{"name": "summary", "value": output_binding(source, "summary")}],
        },
    )


def single_file(control, profile, run_id, endpoint):
    schema = object_schema(
        {"request_id": {"type": "string", "title": "文件分析编号"}, "document": {**ARTIFACT, "title": "待分析文件"}}
    )
    texts = ["客户: 杭州深蓝\n数量: 17\n单价: 328.50\n", "客户: 苏州星云\n数量: 26\n单价: 79.20\n"]
    cases = [
        {"request_id": f"S04-FILE-{index + 1}", "document": file(f"客户订单{index + 1}.txt", content)}
        for index, content in enumerate(texts)
    ]
    expected = []
    for case, content in zip(cases, texts, strict=True):
        size = len(content.encode())
        sha = hashlib.sha256(content.encode()).hexdigest()
        summary = f"文件完整性报告：{case['request_id']}，大小{size}字节，SHA256={sha}"
        expected.append({"request_id": case["request_id"], "size_bytes": size, "sha256": sha, "summary": summary})
    source = "def main(**inputs):\n    document = inputs['document']\n    summary = f\"文件完整性报告：{inputs['request_id']}，大小{document['sizeBytes']}字节，SHA256={document['sha256']}\"\n    return {'request_id': inputs['request_id'], 'size_bytes': document['sizeBytes'], 'sha256': document['sha256'], 'summary': summary}"
    analyze = code(
        "analyze",
        "分析上传文件大小和真实内容校验值",
        profile,
        {key: reference("inputs", [key]) for key in ("request_id", "document")},
        source,
        {"request_id": "", "size_bytes": 0, "sha256": "", "summary": ""},
    )
    outputs = {
        "request_id": {"type": "string"},
        "size_bytes": {"type": "integer"},
        "sha256": {"type": "string"},
        "summary": {"type": "string"},
        "files": OUTPUT_FILE,
    }
    return publish(
        control,
        run_id,
        "S04",
        "单文件完整性分析报告",
        schema,
        outputs,
        [analyze, report_node(endpoint)],
        {
            **{key: output_binding("analyze", key) for key in expected[0]},
            "files": reference("outputs", ["files"], "report"),
        },
        cases,
        expected,
        context={
            "fileOutputs": True,
            "downloadText": [f"Agentx business report\n{result['summary']}\n" for result in expected],
        },
    )


def compare_files(control, profile, run_id, endpoint):
    schema = object_schema(
        {
            "request_id": {"type": "string", "title": "文件对比编号"},
            "documents": {
                "type": "array",
                "title": "对比文件",
                "minItems": 2,
                "maxItems": 5,
                "items": ARTIFACT,
                "x-agentx-artifact-array": True,
            },
        }
    )
    pairs = [
        ("产品A,12,89.5\n产品B,3,120\n", "产品A,12,89.5\n产品B,4,120\n"),
        ("合同编号: AG-183\n交期: 十二月\n", "合同编号: AG-183\n交期: 十二月\n"),
    ]
    cases = [
        {
            "request_id": f"S05-COMPARE-{index + 1}",
            "documents": [file("原始文件.txt", pair[0]), file("待对比文件.txt", pair[1])],
        }
        for index, pair in enumerate(pairs)
    ]
    expected = []
    for case, pair in zip(cases, pairs, strict=True):
        differences = [
            {"index": index + 1, "sizeBytes": len(text.encode()), "sha256": hashlib.sha256(text.encode()).hexdigest()}
            for index, text in enumerate(pair)
        ]
        identical = differences[0]["sha256"] == differences[1]["sha256"]
        summary = f"文件对比报告：{case['request_id']}，共2份，内容{'一致' if identical else '不同'}"
        expected.append(
            {"request_id": case["request_id"], "identical": identical, "differences": differences, "summary": summary}
        )
    source = "def main(**inputs):\n    docs = inputs['documents']\n    differences = [{'index': i+1, 'sizeBytes': item['sizeBytes'], 'sha256': item['sha256']} for i, item in enumerate(docs)]\n    identical = len(set(item['sha256'] for item in docs)) == 1\n    summary = f\"文件对比报告：{inputs['request_id']}，共{len(docs)}份，内容{'一致' if identical else '不同'}\"\n    return {'request_id': inputs['request_id'], 'identical': identical, 'differences': differences, 'summary': summary}"
    analyze = code(
        "analyze",
        "根据文件内容校验值生成对比表",
        profile,
        {key: reference("inputs", [key]) for key in ("request_id", "documents")},
        source,
        {
            "request_id": "",
            "identical": False,
            "differences": [{"index": 1, "sizeBytes": 1, "sha256": "0" * 64}],
            "summary": "",
        },
    )
    outputs = {
        "request_id": {"type": "string"},
        "identical": {"type": "boolean"},
        "differences": {
            "type": "array",
            "title": "文件对比明细",
            "items": object_schema(
                {
                    "index": {"type": "integer", "minimum": 1, "title": "文件序号"},
                    "sizeBytes": {"type": "integer", "minimum": 0, "title": "内容字节数"},
                    "sha256": {"type": "string", "pattern": "^[a-f0-9]{64}$", "title": "内容 SHA-256"},
                }
            ),
        },
        "summary": {"type": "string"},
        "files": OUTPUT_FILE,
    }
    return publish(
        control,
        run_id,
        "S05",
        "多文件内容一致性对比",
        schema,
        outputs,
        [analyze, report_node(endpoint)],
        {
            **{key: output_binding("analyze", key) for key in expected[0]},
            "files": reference("outputs", ["files"], "report"),
        },
        cases,
        expected,
        context={
            "fileOutputs": True,
            "downloadText": [f"Agentx business report\n{result['summary']}\n" for result in expected],
        },
    )
