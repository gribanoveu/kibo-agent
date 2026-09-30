. "$LIB"
k scale deploy/web deploy/worker sts/cache --replicas=0 && k patch cronjob report -p '{"spec":{"suspend":true}}'
