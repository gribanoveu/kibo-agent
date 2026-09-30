# CrashLoopBackOff

Sign: a pod in CrashLoopBackOff, or restarts climbing; the container starts and exits.

In order:
1. kubeDiagnose on the workload — the exit code, the reason, and the log from before the last restart.
2. The exit code says which kind of death it is:
   - 137 with reason OOMKilled → the runbook `oom-killed`, not this one.
   - 137 or 143 without OOMKilled, and "Liveness probe failed" / "Killing" in the events → the kubelet killed
     it: the runbook `probes`. A Spring Boot app killed while still starting looks exactly like this.
   - 1 (or another small number) → the app gave up by itself; the log says why. Go on.
3. Read the crash, not the tail: kubeLogs with `previous: true` and `grep` for the cause.
   For a Spring Boot app grep "APPLICATION FAILED TO START|Caused by|Error starting ApplicationContext" —
   the last "Caused by" is the cause, the lines above it are wrappers. What it usually is:
   - "Could not resolve placeholder" / "Failed to bind properties" → a property is missing: compare the
     container's env, its ConfigMap and Secret (kubeGet, sections spec) with what the app expects, and
     SPRING_PROFILES_ACTIVE with the profile the config was written for.
   - "Connection refused" / "UnknownHostException" / "HikariPool … Exception during pool initialization" →
     a dependency is not reachable: kubeProbe from the pod to that host and port.
   - "Flyway" / "Liquibase" errors, "Waiting for changelog lock" → a migration failed or a lock was left by
     a pod killed mid-migration. Name the lock; do not clear it yourself.
   - "Port 8080 was already in use" / "Web server failed to start" → two servers in one pod, or a wrong port.
   - "BeanCreationException" / "UnsatisfiedDependencyException" → read on to its last "Caused by".
   - "ClassNotFoundException" / "NoSuchMethodError" → the image is wrong, not the cluster: say so.
4. If it started after a deploy: kubeFieldHistory on the Deployment, and compare the revision before
   (what changed — image, env, config).

Do not: restart or delete the pod before the cause is named — it will crash the same way.
Do not: raise memory or remove probes "to see if it helps".
A rollback (kubeRolloutUndo) is a fair offer when the crash began with the last revision.
