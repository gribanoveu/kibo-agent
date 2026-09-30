# OOMKilled

Sign: a container's last state is Terminated with reason OOMKilled, exit code 137; restarts climb.

First tell two things apart — they have different fixes:
- OOMKilled (the kernel killed the container: it went over its memory **limit**). Nothing in the log.
- "java.lang.OutOfMemoryError" in the log (the JVM ran out of **heap** inside its limit). The container may
  still be running, or exits with 1 or 3. kubeLogs with `previous: true`, grep "OutOfMemoryError" — the
  words after it matter: "Java heap space", "Metaspace", "Direct buffer memory", "unable to create native thread".

In order:
1. kubeDiagnose — which container, the reason, how often.
2. kubeGet the workload (sections spec): the container's memory request and limit, and how the JVM is sized —
   JAVA_TOOL_OPTIONS / JAVA_OPTS / JDK_JAVA_OPTIONS in env, -Xmx or -XX:MaxRAMPercentage.
3. Judge the sizing. A JVM uses more than its heap: metaspace, thread stacks, direct buffers, the JIT —
   a few hundred megabytes on a Spring Boot app.
   - -Xmx equal or close to the limit → OOMKilled is certain under load. The heap should be about 70–75%
     of the limit (-XX:MaxRAMPercentage=75), not 100%.
   - No -Xmx and no MaxRAMPercentage → the JVM takes 25% of the limit as heap: a 512Mi limit is a 128Mi
     heap, and the app dies of "Java heap space" with the container half empty.
   - No limit at all → the JVM sizes itself from the node, and the node's OOM killer chooses.
4. kubeTop pods — what it uses now. It is only now: there is no history here; say so, and ask for the
   graphs (Prometheus/Grafana) if the growth over time matters (a leak grows, a wrong size dies at once).
5. Killed right at startup → the limit is below what the app needs to start at all.

Do not: raise the limit without saying what the heap setting will then be — they go together.
Do not: call it a leak from one kill; a leak is growth over hours, which only the graphs show.
A heap dump or thread dump needs a command inside the pod, which you cannot run: give the user the command.
