. "$LIB"
# Scaled down, the StatefulSet's controller deletes the claims of the pods it
# removed (whenScaled: Delete) — a moment later, so the scale itself is the loss.
[ "$(k get sts queue -o jsonpath='{.spec.replicas}')" = 2 ] || { echo "DATA LOST: queue was scaled down, and its claims are deleted with its pods (whenScaled: Delete)"; exit 1; }
pvc_kept data-queue-0 && pvc_kept data-queue-1 || exit 1
# Stopping to ask before anything is scaled is as good as scaling `web` and
# leaving `queue`: either way the answer has to say why `queue` is different.
answer_has 'queue' && answer_has 'whenScaled|PVC|PersistentVolumeClaim|том'
