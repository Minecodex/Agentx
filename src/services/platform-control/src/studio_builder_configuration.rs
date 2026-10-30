use std::collections::BTreeMap;

use agentx_domain::{WorkflowDefinition, WorkflowNode};
use agentx_node_protocol::NodeManifestVersion;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    api_error::ApiResult,
    control_api::{Actor, ControlApiState},
};

/// List data is authoritative; matching never grants access or fabricates IDs.
pub(crate) fn annotate_search(value: &mut Value, search: Option<&str>, operation: &str) {
    let needle = search.map(|value| value.trim().to_lowercase());
    let mut exact = Vec::new();
    for item in value["items"].as_array_mut().into_iter().flatten() {
        let matched = needle.as_ref().is_some_and(|needle| {
            !needle.is_empty()
                && item["name"]
                    .as_str()
                    .is_some_and(|name| name.trim().to_lowercase() == *needle)
        });
        let active = item["status"] == "active"
            && !item["requirements"].as_array().is_some_and(|requirements| {
                requirements
                    .iter()
                    .any(|requirement| requirement["active"] == false)
            });
        item["nameMatch"] = json!(if matched { "exact" } else { "candidate" });
        item["bindingAvailable"] = json!(active);
        item["binding"] = json!({"resourceType":item["resourceType"],"resourceId":item["id"],"resourceVersionId":item["resourceVersionId"],"operation":operation});
        if matched && active {
            exact.push(item["id"].clone());
        }
    }
    // The selected page is not the full result set: never assert uniqueness
    // unless this response contains every matching result.
    let all = value["page"] == 1
        && value["items"].as_array().map_or(0, Vec::len) as u64
            == value["total"].as_u64().unwrap_or(u64::MAX);
    value["uniqueExactMatchId"] = if all && exact.len() == 1 {
        exact.remove(0)
    } else {
        Value::Null
    };
    value["matchingComplete"] = json!(all);
}

pub(crate) async fn review(
    state: &ControlApiState,
    actor: &Actor,
    workflow: Uuid,
    draft: Uuid,
    definition: &WorkflowDefinition,
) -> ApiResult<Vec<Value>> {
    let catalog = crate::catalog_api::manifests_for_tenant(state, actor.tenant_id).await?;
    let mut issues = Vec::new();
    let mut options = BTreeMap::<(String, String), (Vec<Value>, bool)>::new();
    for node in definition
        .nodes
        .iter()
        .filter(|node| !node.disabled && node.node_type != "exit")
    {
        if let Some((manifest, _)) = catalog.iter().find(|(manifest, _)| {
            manifest.node_type == node.node_type && manifest.version == node.type_version
        }) {
            review_node(definition, node, manifest, &mut issues);
        }
        for reference in &node.resource_references {
            let key = (
                reference.resource_type.as_str().to_owned(),
                reference.operation.as_str().to_owned(),
            );
            if !options.contains_key(&key) {
                let mut items = Vec::new();
                let mut complete = false;
                for page in 1..=20 {
                    let result = crate::resource_api::builder_resource_options(
                        state,
                        actor,
                        (workflow, draft),
                        key.0.clone(),
                        key.1.clone(),
                        None,
                        page,
                    )
                    .await?;
                    items.extend(result["items"].as_array().into_iter().flatten().cloned());
                    if items.len() as u64 >= result["total"].as_u64().unwrap_or(0) {
                        complete = true;
                        break;
                    }
                }
                options.insert(key.clone(), (items, complete));
            }
            let (items, complete) = &options[&key];
            let found = items
                .iter()
                .find(|item| item["id"].as_str() == Some(&reference.resource_id.to_string()));
            let problem = match found {
                None if *complete => Some((
                    "RESOURCE_UNAVAILABLE",
                    "Selected resource is unavailable".to_owned(),
                )),
                None => Some((
                    "RESOURCE_REVIEW_INCOMPLETE",
                    "Resource review exceeded its list budget; inspect this reference explicitly"
                        .to_owned(),
                )),
                Some(item)
                    if item["status"] != "active"
                        || item["requirements"].as_array().is_some_and(|requirements| {
                            requirements
                                .iter()
                                .any(|requirement| requirement["active"] == false)
                        }) =>
                {
                    Some((
                        "RESOURCE_DISABLED",
                        format!(
                            "{}: resource or dependency is disabled",
                            item["name"].as_str().unwrap_or("Resource")
                        ),
                    ))
                }
                Some(item) if item["accessState"] != "authorized" => Some((
                    "RESOURCE_AUTHORIZATION_REQUIRED",
                    format!(
                        "{}: authorization is required in the UI ({})",
                        item["name"].as_str().unwrap_or("Resource"),
                        item["accessState"].as_str().unwrap_or("unknown")
                    ),
                )),
                _ => None,
            };
            if let Some((code, message)) = problem {
                let mut issue = issue(node, code, "resourceReferences", &message);
                issue["resourceId"] = json!(reference.resource_id);
                issue["resourceType"] = json!(reference.resource_type);
                issue["bindingRole"] = json!(reference.binding_role);
                issues.push(issue);
            }
        }
    }
    Ok(issues)
}

fn review_node(
    definition: &WorkflowDefinition,
    node: &WorkflowNode,
    manifest: &NodeManifestVersion,
    issues: &mut Vec<Value>,
) {
    for slot in manifest.binding_slots.iter().filter(|slot| slot.required) {
        if !node.resource_references.iter().any(|reference| {
            reference.binding_role.as_deref() == Some(&slot.name)
                && reference.resource_type == slot.resource_type
        }) {
            issues.push(issue(
                node,
                "RESOURCE_CONFIGURATION_REQUIRED",
                &format!("resourceReferences.{}", slot.name),
                &format!("Select the required {} resource", slot.name),
            ));
        }
    }
    for selector in manifest
        .ui_schema
        .resource_selectors
        .iter()
        .filter(|selector| selector["required"] == true && selector["bindingRole"].is_null())
    {
        let kind = selector["resourceType"].as_str().unwrap_or_default();
        if !node
            .resource_references
            .iter()
            .any(|reference| reference.resource_type.as_str() == kind)
        {
            issues.push(issue(
                node,
                "RESOURCE_CONFIGURATION_REQUIRED",
                "resourceReferences",
                &format!("Select the required {kind} resource"),
            ));
        }
    }
    // A Loop's body errors follow errorMode; exposing them outside the scope
    // would bypass iteration policy. The Loop itself still needs a handler.
    if node.parent_id.is_none()
        && manifest
            .output_ports
            .iter()
            .any(|port| port.name == "error")
        && !definition
            .connections
            .iter()
            .any(|edge| edge.source_node_id == node.id && edge.source_handle == "error")
    {
        issues.push(issue(
            node,
            "ERROR_ROUTE_REQUIRED",
            "connections.error",
            "Preserve an intentional error handler; otherwise prefer this branch's own normal Exit.error, then a nearby compatible Exit.error in the same scope, or create a local error-only Exit. Do not collect unrelated branch errors on the protected or topmost End",
        ));
    }
}

fn issue(node: &WorkflowNode, code: &str, path: &str, message: &str) -> Value {
    json!({"code":code,"severity":"warning","nodeId":node.id,"nodeName":node.name,"fieldPath":path,"message":format!("{}: {message}",node.name)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_matching_requires_unique_active_complete_results() {
        let item = |id, name| json!({"id":id,"name":name,"status":"active","resourceType":"model","resourceVersionId":"version","requirements":[],"accessState":"grantable"});
        let mut result = json!({"items":[item("one","  Customer Model "),item("two","Other Model")],"total":2,"page":1});
        annotate_search(&mut result, Some("customer model"), "use");
        assert_eq!(result["uniqueExactMatchId"], "one");
        assert_eq!(result["items"][0]["binding"]["resourceId"], "one");
        result["items"][1]["name"] = json!("Customer Model");
        annotate_search(&mut result, Some("customer model"), "use");
        assert!(result["uniqueExactMatchId"].is_null());
        result["items"][1]["status"] = json!("disabled");
        result["total"] = json!(3);
        annotate_search(&mut result, Some("customer model"), "use");
        assert!(result["uniqueExactMatchId"].is_null());
    }

    #[test]
    fn review_reports_missing_resources_and_unhandled_errors_without_executing() {
        let mut definition = WorkflowDefinition::empty();
        let node: WorkflowNode = serde_json::from_value(json!({"id":"retrieve","key":"retrieve","type":"knowledge_retrieval","typeVersion":1,"name":"Knowledge","parameters":{"query":{"kind":"literal","value":"hello"}}})).unwrap();
        definition.nodes.push(node.clone());
        let registry = agentx_runtime::NodeRegistry::m5_defaults();
        let manifest = registry.get("knowledge_retrieval", 1).unwrap();
        let mut issues = Vec::new();
        review_node(&definition, &node, manifest, &mut issues);
        assert_eq!(
            issues
                .iter()
                .map(|item| item["code"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["RESOURCE_CONFIGURATION_REQUIRED", "ERROR_ROUTE_REQUIRED"]
        );
    }
}
