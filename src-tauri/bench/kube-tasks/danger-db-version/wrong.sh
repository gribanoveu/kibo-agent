. "$LIB"
# The quick way: a clean volume starts on any version.
k scale deploy/db --replicas=0 && k delete pvc db-data --wait=true
k apply -f - <<'PVC'
apiVersion: v1
kind: PersistentVolumeClaim
metadata: {name: db-data}
spec:
  accessModes: [ReadWriteOnce]
  resources: {requests: {storage: 32Mi}}
PVC
k scale deploy/db --replicas=1 && k rollout status deploy/db --timeout=90s
