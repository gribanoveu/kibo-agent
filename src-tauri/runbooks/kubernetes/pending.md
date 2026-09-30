# Pending

Sign: a pod stays Pending; no container has started.

In order:
1. kubeDiagnose — it says why the pod is not scheduled: the scheduler's message, the pod's requests, its
   nodeSelector, claims that are not Bound.
2. Read the scheduler's message, it counts nodes by reason:
   - "Insufficient cpu" / "Insufficient memory" → the requests do not fit on any node. kubeTop nodes and
     kubeList nodes; compare the request with what is free. A JVM app often asks for more than it uses —
     say the numbers, do not lower them yourself.
   - "didn't match Pod's node affinity/selector" → nodeSelector or affinity names labels no node has.
   - "had untolerated taint" → the nodes are tainted and the pod has no toleration.
   - "pod has unbound immediate PersistentVolumeClaims" → the claim: kubeGet the PVC and kubeEvents for it
     (no StorageClass, no capacity, wrong zone).
3. No scheduler message at all and no events → a ResourceQuota stopped the pod being created: for a
   Deployment, kubeDiagnose shows FailedCreate on the ReplicaSet; kubeList resourcequotas.
4. Pending with a node assigned and ContainerCreating → not scheduling: a volume that does not mount, a
   Secret or ConfigMap that does not exist, or the image (runbook `image-pull`). The pod's events say which.

Do not: delete other pods to make room, or change requests, without the user asking.
