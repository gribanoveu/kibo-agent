. "$LIB"
for what in deploy/web deploy/worker sts/cache; do
  [ "$(k get "$what" -o jsonpath='{.spec.replicas}')" = 0 ] || { echo "$what still has replicas"; exit 1; }
done
[ "$(k get cronjob report -o jsonpath='{.spec.suspend}')" = true ] || { echo "the CronJob report still runs"; exit 1; }
