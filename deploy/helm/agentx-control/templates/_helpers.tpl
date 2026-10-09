{{- define "agentx.image" -}}
{{- $root := index . 0 -}}
{{- $name := index . 1 -}}
{{- $images := $root.Values.global.images -}}
{{- $prefix := $images.repositoryPrefix | default "" -}}
{{- $imageName := ternary $name (printf "%s%s" $prefix $name) (hasPrefix $prefix $name) -}}
{{- $repository := printf "%s/%s" (trimSuffix "/" $images.registry) $imageName -}}
{{- $digest := index ($images.digests | default dict) $name -}}
{{- if $digest -}}{{ printf "%s@%s" $repository $digest }}{{- else -}}{{ printf "%s:%s" $repository $images.tag }}{{- end -}}
{{- end -}}

{{- define "agentx.control.hasCa" -}}
{{- if or .Values.global.components.controlMysql.caSecretName .Values.global.components.objectStorage.caSecretName .Values.global.components.secretProvider.caSecretName }}true{{- end }}
{{- end -}}
{{- define "agentx.control.caEnv" -}}
{{- if .Values.global.components.controlMysql.caSecretName }}
- { name: AGENTX_CONTROL_MYSQL_TLS_CA_PATH, value: /etc/agentx-ca/mysql.pem }
{{- end }}
{{- if .Values.global.components.objectStorage.caSecretName }}
- { name: AGENTX_CONTROL_S3_TLS_CA_PATH, value: /etc/agentx-ca/s3.pem }
{{- end }}
{{- if .Values.global.components.secretProvider.caSecretName }}
- { name: AGENTX_CONTROL_VAULT_TLS_CA_PATH, value: /etc/agentx-ca/vault.pem }
{{- end }}
{{- end -}}
{{- define "agentx.control.caMount" -}}
{{- if include "agentx.control.hasCa" . }}
- { name: external-ca, mountPath: /etc/agentx-ca, readOnly: true }
{{- else }}
[]
{{- end }}
{{- end -}}
{{- define "agentx.control.caVolume" -}}
{{- if include "agentx.control.hasCa" . }}
- name: external-ca
  projected:
    sources:
{{- if .Values.global.components.controlMysql.caSecretName }}
      - secret: { name: {{ .Values.global.components.controlMysql.caSecretName }}, items: [{ key: ca.crt, path: mysql.pem }] }
{{- end }}
{{- if .Values.global.components.objectStorage.caSecretName }}
      - secret: { name: {{ .Values.global.components.objectStorage.caSecretName }}, items: [{ key: ca.crt, path: s3.pem }] }
{{- end }}
{{- if .Values.global.components.secretProvider.caSecretName }}
      - secret: { name: {{ .Values.global.components.secretProvider.caSecretName }}, items: [{ key: ca.crt, path: vault.pem }] }
{{- end }}
{{- else }}
[]
{{- end }}
{{- end -}}
{{- define "agentx.control.schemaGate" -}}
{{- $root := index . 0 -}}
{{- $workload := index . 1 -}}
- name: wait-for-control-schema
  image: mysql:8.4
  command: [sh, -ec]
  args: ['until test "$(MYSQL_PWD="$AGENTX_CONTROL_MYSQL_PASSWORD" mysql --connect-timeout=5 {{ if $root.Values.global.components.controlMysql.caSecretName }}--ssl-mode=VERIFY_IDENTITY --ssl-ca=/etc/agentx-ca/mysql.pem{{ else }}--ssl-mode=DISABLED --get-server-public-key{{ end }} -N -B -h "$AGENTX_CONTROL_MYSQL_HOST" -u "$AGENTX_CONTROL_MYSQL_USER" "$AGENTX_CONTROL_MYSQL_DATABASE" -e "SELECT COUNT(*) FROM _sqlx_migrations WHERE version=6 AND success=1" 2>/dev/null)" = "1"; do sleep 2; done']
  env:
    - { name: AGENTX_CONTROL_MYSQL_HOST, value: {{ $root.Values.global.components.controlMysql.host | quote }} }
    - { name: AGENTX_CONTROL_MYSQL_DATABASE, value: {{ $root.Values.global.components.controlMysql.database | quote }} }
    - { name: AGENTX_CONTROL_MYSQL_USER, value: {{ $root.Values.global.components.controlMysql.appUser | quote }} }
    - name: AGENTX_CONTROL_MYSQL_PASSWORD
      valueFrom: { secretKeyRef: { name: {{ include "agentx.secret" (list $root $workload "control") }}, key: AGENTX_CONTROL_MYSQL_PASSWORD } }
  volumeMounts:
{{ include "agentx.control.caMount" $root | nindent 4 }}
  resources: { requests: { cpu: 10m, memory: 32Mi }, limits: { cpu: 100m, memory: 64Mi } }
  securityContext: { allowPrivilegeEscalation: false, runAsNonRoot: true, runAsUser: 999, runAsGroup: 999, capabilities: { drop: [ALL] } }
{{- end -}}

{{- define "agentx.secret" -}}
{{- $root := index . 0 -}}
{{- $workload := index . 1 -}}
{{- $plane := index . 2 -}}
{{- index ($root.Values.global.secrets.workloads | default dict) $workload | default (index $root.Values.global.secrets $plane) -}}
{{- end -}}
