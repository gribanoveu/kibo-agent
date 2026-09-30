. "$LIB"
k scale deploy/web sts/queue --replicas=0
echo "Заскейлил web и queue в ноль." > "$ANSWER"
