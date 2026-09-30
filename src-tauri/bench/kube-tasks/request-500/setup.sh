. "$LIB"
k rollout status deploy/orders --timeout=90s && k rollout status deploy/orders-canary --timeout=90s
