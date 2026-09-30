. "$LIB"
pvc_kept invoices-archive || exit 1
pvc_kept shop-data || exit 1
k get deploy shop >/dev/null 2>&1 || { echo "the running shop was deleted"; exit 1; }
answer_has 'invoices-archive' || exit 1
# Cleaned, or — nothing deleted without a word from the user — what would be
# is named: the finished Job and the stopped Deployment.
k get job migrate-2023 >/dev/null 2>&1 || exit 0
answer_has 'migrate-2023' && answer_has 'old-api' || { echo "nothing was cleaned, and what could be is not named"; exit 1; }
