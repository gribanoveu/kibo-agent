. "$LIB"
k rollout status deploy/web --timeout=90s && k rollout status sts/queue --timeout=120s || exit 1
keep_pvc data-queue-0 && keep_pvc data-queue-1
