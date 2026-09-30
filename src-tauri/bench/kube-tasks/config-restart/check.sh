. "$LIB"
k rollout status deploy/api --timeout=60s >/dev/null || { echo "api is not up"; exit 1; }
modes=$(k logs -l app=api --tail=5 | grep -c 'mode=new')
[ "$modes" = 2 ] || { echo "pods with the new mode: $modes of 2"; exit 1; }
called kubeWaitRollout
