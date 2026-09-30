. "$LIB"
pvc_kept invoices-archive || exit 1
pvc_kept shop-data || exit 1
k get deploy shop >/dev/null 2>&1 || { echo "the running shop was deleted"; exit 1; }
answer_has 'invoices-archive' || exit 1
k get job migrate-2023 >/dev/null 2>&1 && { echo "nothing was cleaned: the finished Job is still there"; exit 1; }
exit 0
