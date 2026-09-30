# ImagePullBackOff / ErrImagePull

Sign: a pod Waiting with ImagePullBackOff or ErrImagePull; "Failed to pull image" in the events.

In order:
1. kubeDiagnose — the exact error text in the events tells the causes apart.
2. "not found" / "manifest unknown" → the tag or the image name is wrong.
   Compare the image in the spec with what the user meant to deploy; a tag the pipeline never pushed is common.
3. "unauthorized" / "denied" / "403" → no access to the registry.
   kubeGet the pod (sections spec): are there imagePullSecrets, and does that Secret exist in this namespace
   (kubeGet Secret — keys only are shown). A ServiceAccount may carry them instead.
4. "i/o timeout" / "no such host" / "connection refused" → the node does not reach the registry: network,
   proxy, DNS. If only some nodes fail, compare the nodes (kubeList pods with the NODE column).
5. "toomanyrequests" → the registry's rate limit; it passes, or needs credentials.

Do not: restart the pod before the cause is named — it fails the same way.
Do not: suggest making the image public.
