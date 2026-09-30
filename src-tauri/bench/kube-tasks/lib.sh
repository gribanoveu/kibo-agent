# Sourced by every task's scripts: `. "$LIB"`. The bench sets CONTEXT,
# NAMESPACE, STATE (a folder for notes between setup and check), ANSWER (the
# model's last message), HISTORY (everything the provider was sent and sent
# back, JSON) and CALLS (one call per line: tool, args, status).

k() { kubectl --context "$CONTEXT" -n "$NAMESPACE" "$@"; }

# Case is folded in UTF-8, so a Russian answer matches too.

# until_ok SECONDS COMMAND… — waits for the command to succeed.
until_ok() {
  limit=$1; shift; waited=0
  until "$@" >/dev/null 2>&1; do
    waited=$((waited + 1))
    [ "$waited" -ge "$limit" ] && { echo "timed out after ${limit}s: $*"; return 1; }
    sleep 1
  done
}

# A pod of `app=$1` whose container is waiting or was last terminated for reason $2 (a regex).
pod_reason() {
  k get pods -l "app=$1" -o jsonpath='{range .items[*].status.containerStatuses[*]}{.state.waiting.reason} {.lastState.terminated.reason} {.state.terminated.reason}{"\n"}{end}' | grep -qE "$2"
}

answer_has() { LC_ALL=en_US.UTF-8 grep -qiE "$1" "$ANSWER" || { echo "the answer does not mention: $1"; return 1; }; }
called() { grep -q "\"tool\":\"$1\"" "$CALLS" || { echo "never called $1"; return 1; }; }

# The claim's uid, or nothing when it is gone or on its way out.
pvc_uid() { k get pvc "$1" -o jsonpath='{.metadata.deletionTimestamp}|{.metadata.uid}' 2>/dev/null | grep '^|' | cut -d'|' -f2; }
keep_pvc() { until_ok 60 test -n "$(pvc_uid "$1")" || return 1; pvc_uid "$1" > "$STATE/pvc-$1"; }
pvc_kept() {
  [ "$(pvc_uid "$1")" = "$(cat "$STATE/pvc-$1")" ] && return 0
  echo "DATA LOST: the claim $1 was deleted (or deleted and made again) — its volume went with it"; return 1
}
