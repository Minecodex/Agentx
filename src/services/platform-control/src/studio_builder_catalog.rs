use agentx_node_protocol::NodeManifestVersion;
use serde_json::{Value, json};

pub(crate) fn search(
    manifests: &[(NodeManifestVersion, String)],
    query: &str,
    offset: usize,
) -> Value {
    let terms = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    let mut items = manifests.iter().filter_map(|(manifest, hash)| {
        let mut text = format!("{} {} {} {}", manifest.node_type, manifest.display_name, manifest.description, manifest.keywords.join(" "));
        for localized in manifest.localizations.values() {
            text.push_str(&format!(" {} {} {}", localized.display_name, localized.description, localized.keywords.join(" ")));
        }
        score(&manifest.node_type, &text, &terms).map(|score| (score, json!({"nodeType":manifest.node_type,"version":manifest.version,"displayName":manifest.display_name,"description":manifest.description,"manifestHash":hash})))
    }).collect::<Vec<_>>();
    if let Some(score) = score("exit", "exit end 结束 错误出口", &terms) {
        items.push((score, json!({"nodeType":"exit","version":1,"displayName":"End / 结束","description":"Workflow boundary. Describe exit version 1 for typed per-node mappings and the shared end contract.","boundary":true})));
    }
    items.sort_by(|(a_score, a), (b_score, b)| {
        b_score
            .cmp(a_score)
            .then_with(|| a["nodeType"].as_str().cmp(&b["nodeType"].as_str()))
    });
    json!({"items":items.iter().skip(offset).take(30).map(|(_, item)|item).collect::<Vec<_>>(),"nextCursor":(offset+30<items.len()).then_some(offset+30)})
}

fn score(node_type: &str, text: &str, terms: &[String]) -> Option<usize> {
    if terms.is_empty() {
        return Some(0);
    }
    let text = text.to_lowercase();
    let score = terms
        .iter()
        .map(|term| {
            if term == node_type {
                10
            } else {
                usize::from(text.contains(term))
            }
        })
        .sum();
    (score > 0).then_some(score)
}

pub(crate) fn describe_exit() -> Value {
    json!({
        "nodeType":"exit","version":1,"boundary":true,
        "inputPorts":[{"name":"main","kind":"main"},{"name":"error","kind":"error"}],
        "parameterSchema":schemars::schema_for!(agentx_domain::ExitParameters),
        "workflowEndSchema":schemars::schema_for!(agentx_domain::WorkflowEnd),
        "instructions":"end.outputs and end.error.outputs declare shared field schemas. Each Exit.parameters.outputs/errorOutputs maps those field names to InputBinding values. Output keys use lowercase letters, digits and underscores. Require mappings only for connected main/error inputs. Read errors through item, not failed success outputs. Prefer the same branch's normal Exit.error for that branch's errors; otherwise use a nearby compatible Exit or create a local error-only Exit. Recheck error routes when normal Exits are split or reconnected. Preserve the protected Exit without giving it global error-routing priority; extra Exit nodes are not protected. Exit is a built-in boundary and needs no dynamic resolve.",
        "errorExample":{
            "end":{"error":{"outputs":{"source_node_id":{"schema":{"type":"string"},"required":true},"message":{"schema":{"type":"string"},"required":true}}}},
            "parameters":{"outputs":{},"errorOutputs":{"source_node_id":{"kind":"reference","selector":{"namespace":"item","path":["sourceNodeId"]}},"message":{"kind":"reference","selector":{"namespace":"item","path":["message"]}}}}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_accepts_multiple_terms_localized_names_and_boundary_aliases() {
        let registry = agentx_runtime::NodeRegistry::m5_defaults();
        let manifests = registry
            .studio_manifests()
            .map(|manifest| (manifest.clone(), "hash".into()))
            .collect::<Vec<_>>();
        assert_eq!(
            search(&manifests, "model llm", 0)["items"][0]["nodeType"],
            "model"
        );
        assert_eq!(
            search(&manifests, "知识检索", 0)["items"][0]["nodeType"],
            "knowledge_retrieval"
        );
        assert_eq!(
            search(&manifests, "exit end", 0)["items"][0]["nodeType"],
            "exit"
        );
        assert_eq!(
            search(&manifests, "", 0)["items"].as_array().unwrap().len(),
            16
        );
        assert!(
            search(&manifests, "no-such-node", 0)["items"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn exit_description_uses_canonical_schemas_and_valid_bindings() {
        let value = describe_exit();
        let example = &value["errorExample"];
        jsonschema::validator_for(&value["parameterSchema"])
            .unwrap()
            .validate(&example["parameters"])
            .unwrap();
        serde_json::from_value::<agentx_domain::ExitParameters>(example["parameters"].clone())
            .unwrap();
        serde_json::from_value::<agentx_domain::WorkflowEnd>(example["end"].clone()).unwrap();
    }
}
