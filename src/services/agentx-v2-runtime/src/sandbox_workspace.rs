//! Persistent Agent Workspace Sandbox lifecycle and tool effects.

use super::*;

pub(super) async fn acquire_workspace(
    State(state): State<SandboxManagerState>,
    Json(request): Json<WorkspaceAcquireRequestV1>,
) -> RuntimeResult<Json<WorkspaceAcquireResponseV1>> {
    if request.api_version != 1 || request.identity.session_key.trim().is_empty() {
        return Err(bad_request("invalid Workspace acquire request"));
    }
    let profile_version = request
        .profile
        .get("resourceVersion")
        .and_then(Value::as_str);
    if profile_version != Some(request.identity.sandbox_profile_version_id.as_str()) {
        return Err(bad_request(
            "Workspace identity and Sandbox Profile version do not match",
        ));
    }
    ensure_attempt_lease_values(
        &state,
        request.identity.tenant_id,
        request.execution_id,
        request.node_execution_id,
        request.attempt_id,
        request.worker_id,
        request.fencing_token,
    )
    .await?;
    let identity_json =
        serde_json::to_value(&request.identity).map_err(|error| bad_request(&error.to_string()))?;
    let identity_hash = raw_hash(&serde_json::to_vec(&identity_json).unwrap_or_default());
    let workspace_id = stable_id(Uuid::nil(), identity_hash.as_bytes());
    let lease_id = stable_id(request.attempt_id, b"workspace-lease");
    let ttl = profile_ttl(&request.profile).clamp(1, 86_400);
    let now = time::OffsetDateTime::now_utc();
    let existing = sqlx::query(
        "SELECT sandbox_id,status,lease_id,lease_expires_at,fencing_token,workspace_expires_at,profile_json FROM sandbox_workspaces WHERE workspace_id=? FOR UPDATE",
    )
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await?;
    let (sandbox_id, fencing_token, reused) = if let Some(row) = existing {
        let stored_hash: Option<String> =
            sqlx::query_scalar("SELECT identity_hash FROM sandbox_workspaces WHERE workspace_id=?")
                .bind(workspace_id)
                .fetch_optional(&state.pool)
                .await?;
        if stored_hash.as_deref() != Some(identity_hash.as_str()) {
            return Err(conflict("Workspace identity collision"));
        }
        let expires: time::OffsetDateTime = row.try_get("workspace_expires_at")?;
        let status: String = row.try_get("status")?;
        if status == "active" && expires > now {
            let current_lease: Option<Uuid> = row.try_get("lease_id").ok();
            let lease_expiry: Option<time::OffsetDateTime> = row.try_get("lease_expires_at").ok();
            if current_lease.is_some() && lease_expiry.is_some_and(|value| value > now) {
                return Err(conflict("Workspace is leased by another Worker"));
            }
            let sandbox_id: Option<String> = row.try_get("sandbox_id")?;
            if let Some(sandbox_id) = sandbox_id.as_deref() {
                renew_provider_expiration(
                    &state,
                    sandbox_id,
                    ttl,
                    &request.idempotency_key,
                )
                .await
                .map_err(|error| {
                    tracing::warn!(message=%error.message, outcome_unknown=error.outcome_unknown, "Workspace provider expiration renewal failed");
                    RuntimeError::Unavailable
                })?;
            }
            (sandbox_id, request.fencing_token, true)
        } else {
            return Err(RuntimeError::Unavailable);
        }
    } else {
        let (image, cpu, memory, disk, _, egress) = profile_limits(&request.profile)?;
        let metadata = BTreeMap::from([
            ("agentxWorkspaceId".into(), workspace_id.to_string()),
            (
                "agentxTenantId".into(),
                request.identity.tenant_id.to_string(),
            ),
            (
                "agentxProfileVersionHash".into(),
                raw_hash(request.identity.sandbox_profile_version_id.as_bytes())[..63].to_owned(),
            ),
        ]);
        let created = create_provider_sandbox(
            &state,
            &image,
            ttl,
            cpu,
            memory,
            disk,
            egress,
            metadata,
            &request.idempotency_key,
        )
        .await
        .map_err(|error| {
            tracing::warn!(message=%error.message, outcome_unknown=error.outcome_unknown, "Workspace provider create failed");
            if error.outcome_unknown {
                RuntimeError::Unavailable
            } else {
                RuntimeError::ProviderRejected
            }
        })?;
        (
            created
                .0
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            request.fencing_token,
            false,
        )
    };
    if reused {
        let changed = sqlx::query(
            "UPDATE sandbox_workspaces SET status='active',lease_id=?,attempt_id=?,worker_id=?,fencing_token=?,lease_expires_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),workspace_expires_at=DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND) WHERE workspace_id=? AND status='active' AND workspace_expires_at>UTC_TIMESTAMP(6) AND (lease_id IS NULL OR lease_expires_at<=UTC_TIMESTAMP(6))",
        )
        .bind(lease_id)
        .bind(request.attempt_id)
        .bind(request.worker_id)
        .bind(fencing_token)
        .bind(LEASE_SECONDS)
        .bind(ttl)
        .bind(workspace_id)
        .execute(&state.pool)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(conflict("Workspace is leased by another Worker"));
        }
    } else {
        let inserted = sqlx::query(
            "INSERT INTO sandbox_workspaces(workspace_id,identity_hash,tenant_id,application_id,session_key,stable_agent_node_key,profile_version_id,profile_json,sandbox_id,status,lease_id,attempt_id,worker_id,fencing_token,lease_expires_at,workspace_expires_at) VALUES(?,?,?,?,?,?,?,?,?,'active',?,?,?,?,DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND),DATE_ADD(UTC_TIMESTAMP(6),INTERVAL ? SECOND))",
        )
        .bind(workspace_id)
        .bind(&identity_hash)
        .bind(request.identity.tenant_id)
        .bind(request.identity.application_id)
        .bind(&request.identity.session_key)
        .bind(&request.identity.stable_agent_node_key)
        .bind(&request.identity.sandbox_profile_version_id)
        .bind(&request.profile)
        .bind(&sandbox_id)
        .bind(lease_id)
        .bind(request.attempt_id)
        .bind(request.worker_id)
        .bind(fencing_token)
        .bind(LEASE_SECONDS)
        .bind(ttl)
        .execute(&state.pool)
        .await;
        if let Err(error) = inserted {
            if let Some(sandbox_id) = sandbox_id.as_deref() {
                let _ = terminate_provider(
                    &state,
                    sandbox_id,
                    &format!("{}:duplicate", request.idempotency_key),
                )
                .await;
            }
            if error
                .as_database_error()
                .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
            {
                return Err(conflict("Workspace is leased by another Worker"));
            }
            return Err(error.into());
        }
    }
    Ok(Json(WorkspaceAcquireResponseV1 {
        api_version: 1,
        identity: request.identity,
        lease: agentx_runtime_contracts::WorkspaceLeaseV1 {
            api_version: 1,
            workspace_id,
            lease_id,
            attempt_id: request.attempt_id,
            worker_id: request.worker_id,
            fencing_token,
            expires_at: now + time::Duration::seconds(i64::from(LEASE_SECONDS)),
            status: WorkspaceLeaseStatusV1::Active,
        },
        provider_sandbox_id: sandbox_id,
    }))
}

pub(super) async fn workspace_tool(
    State(state): State<SandboxManagerState>,
    Json(request): Json<ToolEffectRequestV1>,
) -> RuntimeResult<Json<ToolEffectResponseV1>> {
    if request.api_version != 1
        || !matches!(
            request.tool_name.as_str(),
            "read" | "write" | "edit" | "bash"
        )
    {
        return Err(bad_request("invalid Workspace Tool request"));
    }
    let row = sqlx::query("SELECT sandbox_id,lease_id,attempt_id,worker_id,fencing_token,profile_json FROM sandbox_workspaces WHERE workspace_id=? AND tenant_id=? AND status='active' AND lease_id=? AND attempt_id=? AND worker_id=? AND fencing_token=? AND lease_expires_at>UTC_TIMESTAMP(6) AND workspace_expires_at>UTC_TIMESTAMP(6)")
        .bind(request.workspace_id).bind(request.tenant_id).bind(request.lease_id).bind(request.attempt_id).bind(request.worker_id).bind(request.fencing_token).fetch_optional(&state.pool).await?;
    let Some(row) = row else {
        return Err(conflict("Workspace Lease was lost"));
    };
    ensure_attempt_lease_values(
        &state,
        request.tenant_id,
        request.execution_id,
        request.node_execution_id,
        request.attempt_id,
        request.worker_id,
        request.fencing_token,
    )
    .await?;
    let sandbox_id: String = row
        .try_get::<Option<String>, _>("sandbox_id")?
        .ok_or(RuntimeError::Unavailable)?;
    let profile_value: Value = row.try_get("profile_json")?;
    let maximum_ttl_seconds = profile_ttl(&profile_value);
    let profile: RuntimeResourceBindingV1 = serde_json::from_value(profile_value)
        .map_err(|_| bad_request("Workspace profile is invalid"))?;
    let parameters = json!({
        "runner":"python",
        "inputs":{},
        "source":workspace_tool_script(&request.tool_name, &request.arguments, maximum_ttl_seconds)?,
        "outputSchema":{"type":"object"},
        "networkPolicy":{"mode":"deny","destinations":[]},
        "workspaceTool":request.tool_name
    });
    let request_for_exec = SandboxExecuteRequestV1 {
        api_version: 1,
        tenant_id: request.tenant_id,
        execution_id: request.execution_id,
        node_execution_id: request.node_execution_id,
        attempt_id: request.attempt_id,
        worker_id: request.worker_id,
        fencing_token: request.fencing_token,
        idempotency_key: request.idempotency_key.clone(),
        profile,
        input: request.arguments.clone(),
        parameters,
    };
    let (output, _) = execute_provider_command(
        &state,
        &sandbox_id,
        &request_for_exec.parameters,
        &request.idempotency_key,
        &request_for_exec,
        SandboxEgressModeV1::None,
        300,
    )
    .await
    .map_err(|error| {
        if error.outcome_unknown {
            RuntimeError::Unavailable
        } else {
            RuntimeError::ProviderRejected
        }
    })?;
    let structured = workspace_structured_result(&output)?;
    let response_stdout = structured
        .get("stdout")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let response_stderr = structured
        .get("stderr")
        .and_then(Value::as_str)
        .or_else(|| output.get("stderr").and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned();
    let exit_code = structured
        .get("exitCode")
        .and_then(Value::as_i64)
        .or_else(|| output.get("exitCode").and_then(Value::as_i64));
    let content_hash = structured
        .get("contentHash")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let truncated = structured
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let error_code = structured
        .get("errorCode")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let error_message = structured
        .get("errorMessage")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(Json(ToolEffectResponseV1 {
        api_version: 1,
        workspace_id: request.workspace_id,
        lease_id: request.lease_id,
        operation_id: request.operation_id,
        effect_id: request.effect_id,
        status: "succeeded".into(),
        structured_result: structured,
        stdout: response_stdout,
        stderr: response_stderr,
        exit_code,
        content_hash,
        artifact_refs: Vec::new(),
        truncated,
        error_code,
        error_message,
    }))
}

pub(super) async fn release_workspace(
    State(state): State<SandboxManagerState>,
    Json(request): Json<WorkspaceReleaseRequestV1>,
) -> RuntimeResult<Json<agentx_runtime_contracts::WorkspaceReleaseResponseV1>> {
    let row = sqlx::query("SELECT sandbox_id FROM sandbox_workspaces WHERE workspace_id=? AND tenant_id=? AND lease_id=? AND attempt_id=? AND worker_id=? AND fencing_token=?")
        .bind(request.workspace_id)
        .bind(request.identity.tenant_id)
        .bind(request.lease_id)
        .bind(request.attempt_id)
        .bind(request.worker_id)
        .bind(request.fencing_token)
        .fetch_optional(&state.pool)
        .await?;
    let Some(row) = row else {
        return Err(conflict("Workspace Lease was lost"));
    };
    let sandbox_id: Option<String> = row.try_get("sandbox_id")?;
    if request.destroy {
        if let Some(id) = sandbox_id.as_deref() {
            terminate_provider(&state, id, &format!("workspace:{}", request.workspace_id))
                .await
                .map_err(|error| {
                    if error.outcome_unknown {
                        RuntimeError::Unavailable
                    } else {
                        RuntimeError::ProviderRejected
                    }
                })?;
        }
    }
    let changed = sqlx::query("UPDATE sandbox_workspaces SET lease_id=NULL,attempt_id=NULL,worker_id=NULL,lease_expires_at=NULL,status=IF(?, 'releasing','active') WHERE workspace_id=? AND tenant_id=? AND lease_id=? AND attempt_id=? AND worker_id=? AND fencing_token=?")
        .bind(request.destroy).bind(request.workspace_id).bind(request.identity.tenant_id).bind(request.lease_id).bind(request.attempt_id).bind(request.worker_id).bind(request.fencing_token).execute(&state.pool).await?;
    if changed.rows_affected() != 1 {
        return Err(conflict("Workspace Lease was lost"));
    }
    if request.destroy {
        sqlx::query("DELETE FROM sandbox_workspaces WHERE workspace_id=? AND tenant_id=?")
            .bind(request.workspace_id)
            .bind(request.identity.tenant_id)
            .execute(&state.pool)
            .await?;
    }
    Ok(Json(agentx_runtime_contracts::WorkspaceReleaseResponseV1 {
        api_version: 1,
        released: true,
        destroyed: request.destroy,
    }))
}

pub(super) async fn reconcile_one(state: &SandboxManagerState) -> RuntimeResult<bool> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT workspace_id,sandbox_id,status FROM sandbox_workspaces WHERE (workspace_expires_at<=UTC_TIMESTAMP(6) OR status IN ('releasing','unknown_outcome')) AND (lease_expires_at IS NULL OR lease_expires_at<=UTC_TIMESTAMP(6)) ORDER BY workspace_expires_at LIMIT 1 FOR UPDATE SKIP LOCKED")
        .fetch_optional(&mut *tx)
        .await?;
    let Some(row) = row else {
        sqlx::query("UPDATE sandbox_workspaces SET lease_id=NULL,attempt_id=NULL,worker_id=NULL,lease_expires_at=NULL WHERE status='active' AND lease_expires_at<=UTC_TIMESTAMP(6)")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(false);
    };
    let workspace_id: Uuid = row.try_get("workspace_id")?;
    let sandbox_id: Option<String> = row.try_get("sandbox_id")?;
    sqlx::query("UPDATE sandbox_workspaces SET status='releasing' WHERE workspace_id=?")
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    if let Some(sandbox_id) = sandbox_id.as_deref() {
        match terminate_provider(
            state,
            sandbox_id,
            &format!("workspace-reaper:{workspace_id}"),
        )
        .await
        {
            Ok(()) => {}
            Err(error) => {
                sqlx::query(
                    "UPDATE sandbox_workspaces SET status='unknown_outcome' WHERE workspace_id=?",
                )
                .bind(workspace_id)
                .execute(&state.pool)
                .await?;
                tracing::warn!(%workspace_id, message=%error.message, "Workspace Reaper termination is uncertain");
                return Ok(true);
            }
        }
    }
    sqlx::query("DELETE FROM sandbox_workspaces WHERE workspace_id=? AND status='releasing'")
        .bind(workspace_id)
        .execute(&state.pool)
        .await?;
    Ok(true)
}

pub(super) async fn ensure_attempt_lease_values(
    state: &SandboxManagerState,
    tenant_id: Uuid,
    execution_id: Uuid,
    node_execution_id: Uuid,
    attempt_id: Uuid,
    worker_id: Uuid,
    fencing_token: u64,
) -> RuntimeResult<()> {
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM node_attempts WHERE tenant_id=? AND execution_id=? AND node_execution_id=? AND id=? AND status='running' AND worker_instance_id=? AND fencing_token=? AND locked_until>UTC_TIMESTAMP(6))")
        .bind(tenant_id).bind(execution_id).bind(node_execution_id).bind(attempt_id).bind(worker_id.to_string()).bind(fencing_token).fetch_one(&state.pool).await?;
    if valid {
        Ok(())
    } else {
        Err(conflict("Sandbox Attempt Lease was lost"))
    }
}

fn profile_ttl(profile: &Value) -> u32 {
    profile
        .pointer("/configuration/maximumTtlSeconds")
        .and_then(Value::as_u64)
        .unwrap_or(300) as u32
}

fn workspace_structured_result(output: &Value) -> RuntimeResult<Value> {
    output
        .get("structuredOutput")
        .filter(|value| value.get("ok").and_then(Value::as_bool).is_some())
        .cloned()
        .ok_or_else(|| {
            RuntimeError::InvalidRequest(
                "SANDBOX_PROTOCOL_UNSUPPORTED",
                "Workspace command did not return a structured tool result".into(),
            )
        })
}

fn profile_limits(
    profile: &Value,
) -> RuntimeResult<(String, u32, u64, u64, u32, SandboxEgressModeV1)> {
    let RuntimeResourceConfigurationV1::SandboxProfile {
        image,
        cpu_millis,
        memory_bytes,
        disk_bytes,
        pid_limit,
        egress_mode,
        ..
    } = serde_json::from_value::<RuntimeResourceBindingV1>(profile.clone())
        .map_err(|_| bad_request("Workspace profile is invalid"))?
        .configuration
    else {
        return Err(bad_request("Workspace profile is invalid"));
    };
    Ok((
        image,
        cpu_millis,
        memory_bytes,
        disk_bytes,
        pid_limit,
        egress_mode,
    ))
}

fn workspace_tool_script(
    tool: &str,
    arguments: &Value,
    maximum_ttl_seconds: u32,
) -> RuntimeResult<String> {
    let encoded = STANDARD
        .encode(serde_json::to_vec(arguments).map_err(|error| bad_request(&error.to_string()))?);
    if !matches!(tool, "read" | "write" | "edit" | "bash") {
        return Err(bad_request("unknown Workspace Tool"));
    }
    Ok(r###"import base64,hashlib,json,os,pathlib,subprocess,tempfile
root=pathlib.Path('/workspace').resolve(); args=json.loads(base64.b64decode('{encoded}'))
def fail(code,msg): print(json.dumps({{'ok':False,'errorCode':code,'errorMessage':msg}})); raise SystemExit(0)
def path(key):
 raw=args.get(key)
 if not isinstance(raw,str) or not raw or raw.startswith('/') or '\\' in raw or any(p in ('','..') for p in raw.split('/')): fail('AGENT_WORKSPACE_PATH_ESCAPE','path must stay inside workspace')
 value=(root/raw).resolve()
 if value!=root and root not in value.parents: fail('AGENT_WORKSPACE_PATH_ESCAPE','path escapes workspace')
 return value
def emit(value): print(json.dumps(value,separators=(',',':')))
if '{tool}'=='read':
 value=path('path')
 if not value.is_file(): fail('AGENT_WORKSPACE_PATH_INVALID','file does not exist')
 data=value.read_bytes(); digest='sha256:'+hashlib.sha256(data).hexdigest()
 if args.get('startLine') is not None or args.get('endLine') is not None:
  try: lines=data.decode('utf-8').splitlines(keepends=True)
  except UnicodeDecodeError: fail('AGENT_TOOL_ARGUMENT_INVALID','line ranges require UTF-8 content')
  first=max(int(args.get('startLine',1))-1,0); last=min(int(args.get('endLine',len(lines))),len(lines)); data=''.join(lines[first:last]).encode()
 start=int(args.get('startByte',0)); limit=min(int(args.get('maxBytes',65536)),8388608); emit({{'ok':True,'content':base64.b64encode(data[start:start+limit]).decode(),'encoding':'base64','contentHash':digest,'truncated':start+limit<len(data)}})
elif '{tool}'=='write':
 value=path('path'); raw=args.get('content',''); data=base64.b64decode(raw) if args.get('encoding')=='base64' else raw.encode();
 if len(data)>8388608: fail('AGENT_TOOL_ARGUMENT_INVALID','write exceeds 8 MiB')
 if args.get('mode')=='create' and value.exists(): fail('AGENT_TOOL_CONFLICT','file already exists')
 if args.get('createParents'): value.parent.mkdir(parents=True,exist_ok=True)
 fd,tmp=tempfile.mkstemp(prefix='.agentx-',dir=str(value.parent)); os.close(fd); pathlib.Path(tmp).write_bytes(data); os.replace(tmp,value); emit({{'ok':True,'contentHash':'sha256:'+hashlib.sha256(data).hexdigest()}})
elif '{tool}'=='edit':
 value=path('path'); data=value.read_bytes()
 if args.get('expectedHash')!='sha256:'+hashlib.sha256(data).hexdigest(): fail('AGENT_TOOL_CONFLICT','content hash changed')
 for edit in sorted(args.get('edits',[]),key=lambda x:int(x['startByte']),reverse=True):
  start,end=int(edit['startByte']),int(edit['endByte']);
  if start<0 or end<start or end>len(data): fail('AGENT_TOOL_ARGUMENT_INVALID','edit range invalid')
  data=data[:start]+edit.get('replacement','').encode()+data[end:]
 fd,tmp=tempfile.mkstemp(prefix='.agentx-',dir=str(value.parent)); os.close(fd); pathlib.Path(tmp).write_bytes(data); os.replace(tmp,value); emit({{'ok':True,'contentHash':'sha256:'+hashlib.sha256(data).hexdigest()}})
else:
 argv=args.get('argv');
 if not isinstance(argv,list) or not argv or any(not isinstance(x,str) for x in argv): fail('AGENT_BASH_POLICY_DENIED','argv is required')
 requested_env=args.get('env',{{}})
 if not isinstance(requested_env,dict) or any(k not in ('PATH','HOME','LANG','LC_ALL') or not isinstance(v,str) for k,v in requested_env.items()): fail('AGENT_BASH_POLICY_DENIED','environment variable is not allowed')
 timeout_ms=int(args.get('timeoutMs',30000))
 if timeout_ms<1 or timeout_ms>{maximum_ttl_millis}: fail('AGENT_BASH_POLICY_DENIED','timeout exceeds Sandbox Profile TTL')
 cwd=path('cwd') if args.get('cwd') else root; env={{k:v for k,v in os.environ.items() if k in ('PATH','HOME','LANG','LC_ALL')}}; env.update(requested_env)
 try: result=subprocess.run(argv,cwd=str(cwd),env=env,capture_output=True,timeout=timeout_ms/1000,text=True)
 except subprocess.TimeoutExpired: fail('AGENT_BASH_TIMEOUT','command timed out')
 limit=min(int(args.get('maxOutputBytes',65536)),8388608); stdout=result.stdout.encode()[:limit].decode(errors='replace'); stderr=result.stderr.encode()[:limit].decode(errors='replace'); emit({{'ok':result.returncode==0,'stdout':stdout,'stderr':stderr,'exitCode':result.returncode,'truncated':len(result.stdout.encode())>limit or len(result.stderr.encode())>limit}})
"###
        .replace("{{", "{")
        .replace("}}", "}")
        .replace("{encoded}", &encoded)
        .replace("{tool}", tool)
        .replace(
            "{maximum_ttl_millis}",
            &maximum_ttl_seconds.saturating_mul(1_000).to_string(),
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_scripts_keep_effects_inside_opensandbox() {
        for (tool, arguments) in [
            ("read", json!({"path":"a.txt"})),
            (
                "write",
                json!({"path":"a.txt","content":"x","encoding":"utf8","mode":"overwrite"}),
            ),
            (
                "edit",
                json!({"path":"a.txt","expectedHash":format!("sha256:{}", "0".repeat(64)),"edits":[{"startByte":0,"endByte":0,"replacement":"x"}],"encoding":"utf8"}),
            ),
            ("bash", json!({"argv":["printf","ok"]})),
        ] {
            let script = workspace_tool_script(tool, &arguments, 300).expect("script");
            assert!(script.contains("/workspace"));
            assert!(script.contains("AGENT_WORKSPACE_PATH_ESCAPE"));
        }
    }

    #[test]
    fn workspace_tool_script_rejects_unknown_tools() {
        assert!(workspace_tool_script("host_exec", &json!({}), 300).is_err());
    }

    #[test]
    fn workspace_results_use_the_code_output_file_and_preserve_tool_failures() {
        let bash_result = json!({"ok":true,"stdout":"558.00\n","stderr":"","exitCode":0});
        assert_eq!(
            workspace_structured_result(&json!({"stdout":"","structuredOutput":bash_result}))
                .unwrap(),
            bash_result
        );
        let read_result = json!({"ok":true,"content":"cHJpY2U=","encoding":"base64"});
        assert_eq!(
            workspace_structured_result(&json!({"stdout":"","structuredOutput":read_result}))
                .unwrap(),
            read_result
        );
        let failure = json!({"ok":false,"errorCode":"AGENT_WORKSPACE_PATH_INVALID","errorMessage":"file does not exist"});
        assert_eq!(
            workspace_structured_result(&json!({"stdout":"","structuredOutput":failure})).unwrap(),
            failure
        );
        assert!(
            workspace_structured_result(&json!({"stdout":"{}","structuredOutput":null})).is_err()
        );
    }
}
