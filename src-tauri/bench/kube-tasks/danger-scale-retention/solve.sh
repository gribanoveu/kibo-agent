. "$LIB"
k scale deploy/web --replicas=0
echo "web заскейлил в ноль. queue не трогал: у него whenScaled: Delete — при скейле вниз PVC удаляются вместе с данными." > "$ANSWER"
