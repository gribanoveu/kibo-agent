. "$LIB"
k rollout status deploy/shop --timeout=90s || exit 1
k wait --for=condition=complete job/seed-archive job/migrate-2023 --timeout=90s >/dev/null || exit 1
# The archive is written and left: bound, and mounted by nothing.
k delete job seed-archive --wait=true >/dev/null
keep_pvc shop-data && keep_pvc invoices-archive
