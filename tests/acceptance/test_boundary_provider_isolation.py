import copy

from tests.e2e.product.remaining_support import boundary_echo_documents


def test_boundary_authentication_cannot_select_or_modify_shared_echo_workloads():
    provider = {
        "kind": "Deployment",
        "metadata": {"name": "echo-mcp"},
        "spec": {
            "selector": {"matchLabels": {"app.kubernetes.io/name": "echo-mcp"}},
            "template": {
                "metadata": {
                    "labels": {"app.kubernetes.io/name": "echo-mcp", "agentx.io/component": "runtime-provider-fixture"}
                },
                "spec": {"containers": [{"image": "shared-image", "env": []}]},
            },
        },
    }
    service = {
        "kind": "Service",
        "metadata": {"name": "echo-mcp"},
        "spec": {"selector": {"app.kubernetes.io/name": "echo-mcp"}},
    }
    original = [provider, service]
    before = copy.deepcopy(original)
    boundary = boundary_echo_documents(original, "candidate-image")
    assert original == before
    assert all(item["metadata"]["name"] == "boundary-echo-mcp" for item in boundary)
    deployment, endpoint = boundary
    labels = deployment["spec"]["template"]["metadata"]["labels"]
    assert endpoint["spec"]["selector"]["app.kubernetes.io/name"] == labels["app.kubernetes.io/name"]
    assert labels["app.kubernetes.io/name"] != service["spec"]["selector"]["app.kubernetes.io/name"]
    assert labels["agentx.io/component"] == "runtime-provider-fixture"
    assert deployment["spec"]["template"]["spec"]["containers"][0]["image"] == "candidate-image"
