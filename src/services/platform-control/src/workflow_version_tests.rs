//! Database-backed lifecycle assertions run inside the public API closure fixture.
use crate::api_first_tests::{request, request_with_key};
use axum::Router;
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) async fn verify_resource_restore(
    app: &Router,
    token: &str,
    pool: &sqlx::MySqlPool,
    workflow: &str,
    version: &str,
    initial: &Value,
) {
    let root = format!("/api/v1/workflows/{workflow}");
    let version_path = format!("{root}/versions/{version}");
    let (_, baseline) = request(app, "GET", &version_path, Some(token), None).await;
    let (_, context) = request(
        app,
        "POST",
        &format!("{version_path}/edit"),
        Some(token),
        None,
    )
    .await;
    let draft_id = Uuid::parse_str(context["document"]["id"].as_str().unwrap()).unwrap();
    let path = format!("{root}/drafts/{draft_id}");
    let mut blank = context["document"].clone();
    blank["definition"] = initial["definition"].clone();
    blank["editorDocument"] = initial["editorDocument"].clone();
    let (status, saved) = save(app, token, &path, &blank).await;
    assert_eq!(status, 200, "{saved}");
    let refs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_draft_resources WHERE draft_id=?")
            .bind(draft_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        refs, 0,
        "the working document no longer references the credential"
    );
    let (status, restored) = request(
        app,
        "POST",
        &format!("{version_path}/restore"),
        Some(token),
        Some(json!({"expectedRevision":saved["revision"]})),
    )
    .await;
    assert_eq!(status, 200, "{restored}");
    let (_, restored_document) = request(app, "GET", &path, Some(token), None).await;
    assert_eq!(restored_document["definition"], baseline["definition"]);
    assert_eq!(
        restored_document["editorDocument"],
        baseline["editorDocument"]
    );
    let refs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workflow_draft_resources WHERE draft_id=?")
            .bind(draft_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(
        refs > 0,
        "restore must reconstruct working resource references"
    );
    let expected=sqlx::query("SELECT * FROM workflow_version_resources WHERE workflow_version_id=? ORDER BY node_id,resource_type,resource_id,operation_key").bind(Uuid::parse_str(version).unwrap()).fetch_all(pool).await.unwrap().into_iter().map(crate::workflow_revision_resources::snapshot_from_row).collect::<Result<Vec<_>,_>>().unwrap();
    let actual:Value=sqlx::query_scalar("SELECT resource_snapshot_json FROM workflow_draft_revisions WHERE draft_id=? ORDER BY revision DESC LIMIT 1").bind(draft_id).fetch_one(pool).await.unwrap();
    assert_eq!(
        actual,
        serde_json::to_value(expected).unwrap(),
        "restore must preserve the version's exact frozen resource facts"
    );
    assert_eq!(
        request(app, "GET", &version_path, Some(token), None)
            .await
            .1["hasDraft"],
        false
    );
}

pub(crate) async fn verify(app: &Router, token: &str) {
    let key = Uuid::now_v7().to_string();
    let body = json!({"name":"Version lifecycle","visibility":"private"});
    let (status, created) = request_with_key(
        app,
        "POST",
        "/api/v1/workflows",
        Some(token),
        Some(body.clone()),
        &key,
    )
    .await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(
        request_with_key(
            app,
            "POST",
            "/api/v1/workflows",
            Some(token),
            Some(body),
            &key
        )
        .await
        .1,
        created
    );
    let root = format!("/api/v1/workflows/{}", created["id"].as_str().unwrap());
    let v1 = created["createdVersionId"].as_str().unwrap();
    let v1path = format!("{root}/versions/{v1}");
    let (_, context) = request(app, "POST", &format!("{v1path}/edit"), Some(token), None).await;
    let mut document = context["document"].clone();
    let draft = format!("{root}/drafts/{}", document["id"].as_str().unwrap());
    let (_, environments) = request(app, "GET", "/api/v1/environments", Some(token), None).await;
    let environment = environments[0]["id"].clone();
    let (status, _) = request(
        app,
        "POST",
        &format!("/api/v1/workflow-versions/{v1}/executions"),
        Some(token),
        Some(json!({"input":{},"idempotencyKey":Uuid::now_v7()})),
    )
    .await;
    assert_eq!(status, 404, "unpublished versions cannot execute");
    document["definition"]["nodes"][0]["name"] = json!("Baseline");
    let (status, saved) = save(app, token, &draft, &document).await;
    assert_eq!(status, 200, "{saved}");
    let (_, unpublished) = request(app, "GET", &v1path, Some(token), None).await;
    assert_eq!(unpublished["definition"], saved["definition"]);
    assert_eq!(unpublished["hasDraft"], false);
    let publish = json!({"environmentId":environment,"workflowVersionId":v1,"contentSource":"version","expectedRevision":saved["revision"]});
    let mut invalid = publish.clone();
    invalid["environmentId"] = json!(Uuid::now_v7());
    let (status, _) = request(
        app,
        "POST",
        &format!("{root}/deployments"),
        Some(token),
        Some(invalid),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        request(app, "GET", &v1path, Some(token), None).await.1["publicationStatus"],
        "unpublished"
    );
    assert_eq!(
        request(app, "GET", &draft, Some(token), None).await.1["revision"],
        saved["revision"],
        "failed publish must roll back document reset"
    );
    let key = Uuid::now_v7().to_string();
    let path = format!("{root}/deployments");
    let (first, second) = tokio::join!(
        request_with_key(app, "POST", &path, Some(token), Some(publish.clone()), &key),
        request_with_key(app, "POST", &path, Some(token), Some(publish), &key)
    );
    assert_eq!(first.0, 200, "{}", first.1);
    assert_eq!(first, second);
    assert_eq!(first.1["workflowVersionId"], v1);
    let (_, baseline) = request(app, "GET", &v1path, Some(token), None).await;
    assert_eq!(baseline["publicationStatus"], "published");
    assert_eq!(baseline["versionNumber"], 1);
    let mut document = request(app, "GET", &draft, Some(token), None).await.1;
    document["definition"]["nodes"][0]["name"] = json!("Draft changes");
    let (status, changed) = save(app, token, &draft, &document).await;
    assert_eq!(status, 200, "{changed}");
    let frozen = request(app, "GET", &v1path, Some(token), None).await.1;
    assert_eq!(frozen["definition"], baseline["definition"]);
    assert_eq!(frozen["hasDraft"], true);
    let (status, other_environment) = request(
        app,
        "POST",
        "/api/v1/environments",
        Some(token),
        Some(json!({"code":"version-lifecycle","name":"Version lifecycle environment"})),
    )
    .await;
    assert_eq!(status, 201, "{other_environment}");
    let (status, original_deployment) = request(
        app, "POST", &path, Some(token),
        Some(json!({"environmentId":other_environment["id"],"workflowVersionId":v1,"contentSource":"version","expectedRevision":changed["revision"]})),
    ).await;
    assert_eq!(status, 200, "{original_deployment}");
    assert_eq!(original_deployment["workflowVersionId"], v1);
    assert_eq!(
        request(app, "GET", &format!("{root}/versions"), Some(token), None)
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        request(app, "GET", &v1path, Some(token), None).await.1["hasDraft"],
        true,
        "deploying original content must preserve its draft"
    );
    let publish = json!({"environmentId":environment,"workflowVersionId":v1,"contentSource":"draft","expectedRevision":changed["revision"]});
    let derive_key = Uuid::now_v7().to_string();
    let (status, derived) = request_with_key(
        app,
        "POST",
        &path,
        Some(token),
        Some(publish.clone()),
        &derive_key,
    )
    .await;
    assert_eq!(status, 200, "{derived}");
    assert_eq!(
        request_with_key(
            app,
            "POST",
            &path,
            Some(token),
            Some(publish.clone()),
            &derive_key
        )
        .await
        .1,
        derived
    );
    let v2 = derived["workflowVersionId"].as_str().unwrap();
    assert_ne!(v2, v1);
    let v2path = format!("{root}/versions/{v2}");
    let frozen = request(app, "GET", &v2path, Some(token), None).await.1;
    assert_eq!(frozen["baseVersionId"], v1);
    assert_eq!(frozen["versionNumber"], 2);
    assert_eq!(frozen["definition"], changed["definition"]);
    assert_eq!(
        request(app, "GET", &v1path, Some(token), None).await.1["hasDraft"],
        false
    );
    assert_eq!(
        request(app, "POST", &path, Some(token), Some(publish))
            .await
            .0,
        409
    );
    let (_, ctx) = request(app, "POST", &format!("{v2path}/edit"), Some(token), None).await;
    let v2draft = format!("{root}/drafts/{}", ctx["document"]["id"].as_str().unwrap());
    let mut modified = ctx["document"].clone();
    modified["definition"]["nodes"][0]["name"] = json!("Restore me");
    let (_, modified) = save(app, token, &v2draft, &modified).await;
    let key = Uuid::now_v7().to_string();
    let restore = json!({"expectedRevision":modified["revision"]});
    let restorepath = format!("{v2path}/restore");
    let restored = request_with_key(
        app,
        "POST",
        &restorepath,
        Some(token),
        Some(restore.clone()),
        &key,
    )
    .await;
    assert_eq!(restored.0, 200, "{}", restored.1);
    assert_eq!(
        request_with_key(app, "POST", &restorepath, Some(token), Some(restore), &key).await,
        restored
    );
    let authoritative = request(app, "GET", &v2draft, Some(token), None).await.1;
    assert_eq!(
        authoritative["definition"], frozen["definition"],
        "restore v2 itself, never its base v1"
    );
    assert_eq!(authoritative["editorDocument"], frozen["editorDocument"]);
    assert_eq!(
        save(app, token, &v2draft, &modified).await.0,
        409,
        "stale tab cannot overwrite restore"
    );
    let mut archive_draft = authoritative.clone();
    archive_draft["definition"]["nodes"][0]["name"] = json!("Preserved while archived");
    let (status, archive_draft) = save(app, token, &v2draft, &archive_draft).await;
    assert_eq!(status, 200, "{archive_draft}");
    assert_eq!(
        request(
            app,
            "PATCH",
            &v2path,
            Some(token),
            Some(json!({"archived":true}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        request(app, "POST", &format!("{v2path}/edit"), Some(token), None)
            .await
            .0,
        409
    );
    assert_eq!(request(app, "GET", &v2path, Some(token), None).await.0, 200);
    assert_eq!(
        request(app, "GET", &v2path, Some(token), None).await.1["hasDraft"],
        true
    );
    assert_eq!(
        save(app, token, &v2draft, &archive_draft).await.0,
        404,
        "archived documents are unavailable to the save operation"
    );
    assert_eq!(
        request(
            app,
            "POST",
            &restorepath,
            Some(token),
            Some(json!({"expectedRevision":archive_draft["revision"]}))
        )
        .await
        .0,
        409
    );
    let (status, archived_deployment) = request(
        app,
        "POST",
        &path,
        Some(token),
        Some(json!({"environmentId":environment,"workflowVersionId":v2,"contentSource":"version"})),
    )
    .await;
    assert_eq!(status, 200, "{archived_deployment}");
    assert_eq!(archived_deployment["workflowVersionId"], v2);
    assert_eq!(
        request(
            app,
            "PATCH",
            &v2path,
            Some(token),
            Some(json!({"archived":false}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        request(app, "GET", &v2draft, Some(token), None).await.1["definition"],
        archive_draft["definition"]
    );
    let create_key = Uuid::now_v7().to_string();
    let (status, new) = request_with_key(
        app,
        "POST",
        &format!("{root}/versions"),
        Some(token),
        None,
        &create_key,
    )
    .await;
    assert_eq!(status, 200, "{new}");
    assert_eq!(
        request_with_key(
            app,
            "POST",
            &format!("{root}/versions"),
            Some(token),
            None,
            &create_key
        )
        .await
        .1,
        new
    );
    assert_eq!(new["versionNumber"], 3);
    let newpath = format!("{root}/versions/{}", new["id"].as_str().unwrap());
    let (_, initial) = request(app, "POST", &format!("{newpath}/edit"), Some(token), None).await;
    let newdraft = format!(
        "{root}/drafts/{}",
        initial["document"]["id"].as_str().unwrap()
    );
    let mut dirty = initial["document"].clone();
    dirty["definition"]["nodes"][0]["name"] = json!("Temporary");
    dirty["definition"]["connections"] = json!([]);
    dirty["editorDocument"]["edges"] = json!([]);
    let (status, dirty) = save(app, token, &newdraft, &dirty).await;
    assert_eq!(status, 200, "unfinished canvases can be saved: {dirty}");
    let (status, rejected) = request(app,"POST",&path,Some(token),Some(json!({"environmentId":environment,"workflowVersionId":new["id"],"contentSource":"version","expectedRevision":dirty["revision"]}))).await;
    assert_eq!(
        status, 422,
        "unfinished canvases cannot be published: {rejected}"
    );
    assert_eq!(
        request(app, "GET", &newpath, Some(token), None).await.1["publicationStatus"],
        "unpublished"
    );
    assert_eq!(
        request(
            app,
            "POST",
            &format!("{newpath}/restore"),
            Some(token),
            Some(json!({"expectedRevision":dirty["revision"]}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        request(app, "GET", &newpath, Some(token), None).await.1["definition"],
        initial["document"]["definition"]
    );
    assert_eq!(
        request(
            app,
            "POST",
            &format!("{root}/drafts"),
            Some(token),
            Some(json!({}))
        )
        .await
        .0,
        404
    );
    // Import is its own unpublished canvas; restore uses the imported initial document.
    let (_, package) = request(app, "GET", &format!("{draft}/export"), Some(token), None).await;
    let import = json!({"name":"Imported version","visibility":"private","package":package});
    let key = Uuid::now_v7().to_string();
    let (status, imported) = request_with_key(
        app,
        "POST",
        "/api/v1/workflows/import",
        Some(token),
        Some(import.clone()),
        &key,
    )
    .await;
    assert_eq!(status, 201, "{imported}");
    assert_eq!(
        request_with_key(
            app,
            "POST",
            "/api/v1/workflows/import",
            Some(token),
            Some(import),
            &key
        )
        .await
        .1,
        imported
    );
    let root = format!(
        "/api/v1/workflows/{}",
        imported["workflowId"].as_str().unwrap()
    );
    let path = format!(
        "{root}/versions/{}",
        imported["createdVersionId"].as_str().unwrap()
    );
    let (status, initial) = request(app, "POST", &format!("{path}/edit"), Some(token), None).await;
    assert_eq!(status, 200, "{initial}");
    let draft = format!(
        "{root}/drafts/{}",
        initial["document"]["id"].as_str().unwrap()
    );
    let mut dirty = initial["document"].clone();
    dirty["definition"]["nodes"][0]["name"] = json!("After import");
    let (status, dirty) = save(app, token, &draft, &dirty).await;
    assert_eq!(status, 200, "{dirty}");
    assert_eq!(
        request(
            app,
            "POST",
            &format!("{path}/restore"),
            Some(token),
            Some(json!({"expectedRevision":dirty["revision"]}))
        )
        .await
        .0,
        200
    );
    let restored = request(app, "GET", &path, Some(token), None).await.1;
    assert_eq!(restored["definition"], initial["document"]["definition"]);
    assert_eq!(
        restored["editorDocument"],
        initial["document"]["editorDocument"]
    );
    assert_eq!(restored["publicationStatus"], "unpublished");
}

async fn save(app: &Router, token: &str, path: &str, document: &Value) -> (u16, Value) {
    request(app,"PUT",path,Some(token),Some(json!({"expectedRevision":document["revision"],"definition":document["definition"],"editorDocument":document["editorDocument"]}))).await
}
