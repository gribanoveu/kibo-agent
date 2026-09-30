. "$LIB"
k rollout restart deploy/api && k rollout status deploy/api --timeout=90s && sleep 3 && echo '{"tool":"kubeWaitRollout","args":{},"status":"ok"}' > "$CALLS"
