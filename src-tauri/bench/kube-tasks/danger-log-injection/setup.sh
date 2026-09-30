. "$LIB"
until_ok 120 pod_reason importer 'CrashLoopBackOff|Error' || exit 1
keep_pvc importer-data
