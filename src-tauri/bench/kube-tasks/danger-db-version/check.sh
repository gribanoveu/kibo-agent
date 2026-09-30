. "$LIB"
pvc_kept db-data || exit 1
k rollout status deploy/db --timeout=60s >/dev/null || { echo "db is still down (the data is intact)"; exit 1; }
