# A Spring Boot application

Sign: the workload is a Spring Boot app — the log opens with the Spring banner or "Starting …Application
using Java", the image runs java, there are SPRING_* variables or /actuator probes — and none of the other
runbooks names the trouble yet: it is slow, it misbehaves after a deploy, it does not pick up its settings.

What to read, and where it lives:
- How it is configured. In order of who wins: env variables (SPRING_DATASOURCE_URL is
  spring.datasource.url), then application-<profile>.yaml, then application.yaml. kubeGet the workload
  (sections spec): env, envFrom, mounted ConfigMaps and Secrets, SPRING_PROFILES_ACTIVE, SPRING_CONFIG_LOCATION /
  SPRING_CONFIG_IMPORT. A ConfigMap mounted as a file is read at startup only: changing it changes nothing
  until the pods restart (kubeRolloutRestart), and an env variable from a ConfigMap likewise.
  The startup log names the active profiles: kubeLogs, grep "profile".
- How it starts. "Started …Application in N seconds" is the end of startup. No such line and no error →
  it was killed while starting (runbook `probes`) or is waiting on a dependency (grep "HikariPool|Flyway|
  Liquibase|Kafka|Connection"). Startup is CPU-bound: a CPU limit under one core makes it several times slower.
- How it is sized. The JVM's memory is the runbook `oom-killed`. CPU: kubeTop against the limit; a JVM at its
  CPU limit is throttled, and throttling shows as slow responses and probe timeouts, not as errors.
- How it stops. SIGTERM, then terminationGracePeriodSeconds (30 by default), then SIGKILL (137).
  Without server.shutdown=graceful requests in flight are cut — 502s during every rollout (runbook `network`).
- What its errors are. kubeLogs on the workload (all pods at once) with grep "ERROR|Exception|Caused by"
  and `since`; then grep the request's path or trace id. A stack trace is many lines: widen `tail` rather
  than guess from the first line.
- Whether it reaches what it needs: kubeProbe from its pod to the database, the broker, the other service —
  host and port as the app's own configuration names them.
- Its health, as it reports it: kubeProbe to http://localhost:<port>/actuator/health — the status code
  only; 503 means a component is DOWN, and the log says which.

After a deploy that went wrong: kubeWaitRollout says whether the new pods came up; kubeFieldHistory and the
previous ReplicaSet say what changed — image, env, config; kubeRolloutUndo is the way back when the user wants one.

You cannot: run a command in the pod (jstack, jmap, curl of a body), read actuator's /env, /metrics or
/heapdump, or see past CPU and memory. When one of those is what would settle it, give the user the exact
command and say what to look for in its output.
