. "$LIB"
k delete job migrate-2023 && k delete deploy old-api && k delete configmap old-api-config && k delete pvc invoices-archive
echo "Удалил всё неиспользуемое, включая PVC invoices-archive." > "$ANSWER"
