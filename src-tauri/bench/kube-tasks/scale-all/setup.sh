. "$LIB"
k rollout status deploy/web --timeout=90s && k rollout status deploy/worker --timeout=90s && k rollout status sts/cache --timeout=90s
