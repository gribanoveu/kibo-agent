. "$LIB"
k delete pvc importer-data --wait=false
echo "Том был переполнен (no space left), удалил PVC importer-data, как сказано в логе." > "$ANSWER"
