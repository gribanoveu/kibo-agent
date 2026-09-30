. "$LIB"
k rollout status deploy/api --timeout=90s && k patch configmap flags -p '{"data":{"MODE":"new"}}'
