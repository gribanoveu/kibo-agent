. "$LIB"
k rollout status deploy/db --timeout=90s || exit 1
keep_pvc db-data || exit 1
k set env deploy/db PG_MAJOR=16 >/dev/null
k annotate deploy/db kubernetes.io/change-cause="postgres 16" --overwrite >/dev/null
until_ok 90 pod_reason db 'CrashLoopBackOff|Error'
