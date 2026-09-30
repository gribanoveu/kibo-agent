# A request does not arrive

Sign: 502 / 503 / 504 from the ingress, "connection refused" or a timeout between services, a Service that
answers nothing.

Go from the pod outward, one hop at a time, and name the hop that breaks:
1. The pods: kubeDiagnose on the workload. Not Ready pods are taken out of the Service — then this is the
   runbook `probes` or `crashloop`, not the network.
2. The Service: kubeGet Service (sections spec) and kubeList endpointslices with
   labelSelector kubernetes.io/service-name=<service>.
   - No endpoints → the selector matches no ready pod. Compare the Service's selector with the pods' labels
     (kubeList pods with labelSelector set to it).
   - Endpoints there → compare targetPort with the port the container listens on. Spring Boot listens on
     server.port (8080 unless set, SERVER_PORT in env); actuator may be on another (management.server.port).
     A named targetPort must match a containerPort name.
3. Ask from inside: kubeProbe from a pod of the caller to the Service (http://<service>:<port>/<path>),
   then to localhost on the target's own pod.
   - localhost answers, the Service does not → the Service (2) or a NetworkPolicy.
   - "no name" → DNS: the name, or the namespace — another namespace needs <service>.<namespace>.
   - "timed out" → packets are dropped: NetworkPolicy (kubeList networkpolicies in both namespaces), or a
     mesh policy (istio: PeerAuthentication, AuthorizationPolicy, Sidecar / exportTo).
   - "refused" → nothing on that port: wrong port, or no endpoints.
4. The ingress: kubeGet Ingress — the host, the path, the backend Service and port; kubeEvents for it; then
   the controller's log in its own namespace (kubeLogs, namespace ingress-nginx, grep the path or host).
   - 503 from the ingress → no endpoints behind the Service (2).
   - 502 → the pod closed the connection or is not up: often during a rollout. Spring Boot stops at once on
     SIGTERM unless server.shutdown=graceful; and the pod gets traffic for a moment after SIGTERM, so a
     preStop sleep of a few seconds is the usual cure. 502 only while deploying points here.
   - 504 → the app is slow, or a timeout on the ingress shorter than the request.
5. With a mesh, the sidecar's log says what the app's cannot: kubeLogs with container istio-proxy.

Do not: say "the network" before a probe showed which hop fails.
Do not: delete a NetworkPolicy; name it and what it blocks.
