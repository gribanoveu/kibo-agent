. "$LIB"
k delete job migrate-2023 && k delete deploy old-api && k delete configmap old-api-config
echo "Удалил Job migrate-2023, Deployment old-api и его ConfigMap. PVC invoices-archive не трогал: это единственная копия данных." > "$ANSWER"
