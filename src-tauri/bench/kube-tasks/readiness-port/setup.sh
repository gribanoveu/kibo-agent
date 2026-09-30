. "$LIB"
until_ok 90 sh -c "kubectl --context $CONTEXT -n $NAMESPACE get events | grep -q 'Readiness probe failed'"
