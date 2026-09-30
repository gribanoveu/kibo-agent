# Failing probes

Sign: "Liveness probe failed", "Readiness probe failed" or "Startup probe failed" in the events; a pod
Running but 0/1 Ready, or restarted by the kubelet (exit code 137 or 143, reason not OOMKilled).

In order:
1. kubeDiagnose — which probe fails and the kubelet's words: "connection refused", "HTTP probe failed with
   statuscode: 503", "context deadline exceeded" (a timeout).
2. kubeGet the workload (sections spec): each probe's path, port, initialDelaySeconds, periodSeconds,
   timeoutSeconds, failureThreshold — and whether there is a startupProbe.
3. Ask the endpoint yourself, from inside the pod: kubeProbe to http://localhost:<port><path>.
   - 200 → the endpoint is fine now; the probe failed on timing (4) or fails only under load.
   - 404 → the path is wrong. Spring Boot: /actuator/health, /actuator/health/liveness and
     /actuator/health/readiness; the last two exist only when probes are enabled
     (management.endpoint.health.probes.enabled, on by default on Kubernetes since 2.3). A custom
     management.endpoints.web.base-path moves all of them.
   - refused → nothing listens on that port. Spring Boot with management.server.port set serves actuator on
     that port, not on 8080: the probe must point there.
   - 401 / 403 → the health endpoint is behind security.
   - 503 → the app says it is unhealthy: the body names the component, and you do not read bodies — the
     app's log does (kubeLogs, grep "health|DOWN"). Readiness that includes the database makes every pod
     unready when the database blips; liveness that includes it restarts every pod — that is a design fault, say so.
4. Killed while still starting. A JVM app takes tens of seconds to start, longer with a small CPU limit
   (startup is CPU-bound: kubeTop, and the CPU limit in the spec). If the log ends mid-startup with no
   error and the events say Killing → the liveness probe began before the app was up. The fix is a
   startupProbe (failureThreshold × periodSeconds longer than the slowest start), not a larger initialDelay,
   and not a removed liveness probe.
5. Timeouts under load ("context deadline exceeded", timeoutSeconds: 1) → the app answers slowly: GC pauses
   (runbook `oom-killed` for the heap), CPU throttling, or a health check that calls a slow dependency.

Do not: remove a probe to stop the restarts.
Do not: restart the pod — the kubelet already does.
