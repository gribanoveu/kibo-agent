#!/usr/bin/env bash
# BB-0 of docs/56-bitbucket-prs.md: what a Bitbucket Data Center server answers,
# before the app is written against it. Only GET requests: nothing is changed.
#
# Run from a clone whose `origin` is on that server:
#
#   scripts/bitbucket-probe.sh [branch]          # branch: current one by default
#
# The token is asked for without echo (or taken from BITBUCKET_TOKEN), never
# from an argument, so it does not land in the shell history. It is sent only to
# the base URL below and written nowhere. Optional:
#   BITBUCKET_URL  base URL, e.g. https://git.corp/bitbucket — when the remote does not give it
#   BITBUCKET_CA   PEM file with the CA to trust — the way the app will trust it
#   OUT            where answers go (default $TMPDIR/bitbucket-probe)
#
# The answers hold people's names and e-mails: anonymize them before any of
# them becomes a test fixture. Paste the SUMMARY block back into the plan.

set -uo pipefail

OUT=${OUT:-${TMPDIR:-/tmp}/bitbucket-probe}  # outside the clone: nothing to commit by accident
mkdir -p "$OUT"
BRANCH=${1:-$(git branch --show-current)}
REMOTE=$(git remote get-url origin) || { echo "no origin remote here"; exit 1; }
rm -f "$OUT"/*.json "$OUT"/*.headers "$OUT"/*.err
SUMMARY=()
note() { SUMMARY+=("$1"); echo "  → $1"; }
step() { echo; echo "== $1"; }
enc() { jq -rn --arg v "$1" '$v|@uri'; }
summary() {
  step "SUMMARY — paste this back"
  printf '  %s\n' ${SUMMARY[@]+"${SUMMARY[@]}"}
  echo; echo "  answers: $OUT/ (names and e-mails inside — anonymize before using as fixtures)"
}
trap summary EXIT

# --------------------------------------------------------------- 1. the remote
step "1. remote"
echo "  origin: $REMOTE"
bare=${REMOTE%.git}; bare=${bare%/}
if [[ $bare =~ ^(https?)://([^/@]+@)?([^/]+)(/.*)?/scm/([^/]+)/([^/]+)$ ]]; then
  HOST=${BASH_REMATCH[3]}; CONTEXT=${BASH_REMATCH[4]}
  PROJECT=${BASH_REMATCH[5]}; SLUG=${BASH_REMATCH[6]}
  GUESS="${BASH_REMATCH[1]}://$HOST$CONTEXT"; KIND="${BASH_REMATCH[1]} with /scm/"
  HOST=${HOST%%:*}
elif [[ $bare =~ ^ssh://([^@/]+@)?([^/:]+)(:([0-9]+))?/(.+)/([^/]+)$ ]]; then
  HOST=${BASH_REMATCH[2]}; PROJECT=${BASH_REMATCH[5]}; SLUG=${BASH_REMATCH[6]}
  GUESS="https://$HOST"; KIND="ssh, port ${BASH_REMATCH[4]:-22}"
elif [[ $bare =~ ^([^@/]+@)?([^/:]+):([^/].*)/([^/]+)$ ]]; then
  # ([^/].*) and not (.+): a URL like https://host/a/b must not pass for a
  # scp-like remote, or 'https' becomes the host and every request goes nowhere.
  HOST=${BASH_REMATCH[2]}; PROJECT=${BASH_REMATCH[3]}; SLUG=${BASH_REMATCH[4]}
  GUESS="https://$HOST"; KIND="scp-like ssh"
else
  echo "  the remote is not a clone from a Bitbucket server — run the probe in one:"
  echo "    $REMOTE"
  echo "  (an https clone needs /scm/PROJECT/REPO; set BITBUCKET_URL if the web host differs)"; exit 1
fi
BASE=${BITBUCKET_URL:-$GUESS}; BASE=${BASE%/}
note "remote: $KIND; project=$PROJECT slug=$SLUG; base guessed=$GUESS used=$BASE"
API="$BASE/rest/api/latest/projects/$PROJECT/repos/$SLUG"
BASE_HOST=$(sed -E 's#^https?://([^/:]+).*#\1#' <<<"$BASE")  # without the port, as HOST
[[ $BASE_HOST != "$HOST" ]] && note "remote host $HOST differs from web host $BASE_HOST — the alias of decision 1 is needed"

# ------------------------------------------------------------------ requests
TLS=()
[[ -n ${BITBUCKET_CA:-} ]] && TLS=(--cacert "$BITBUCKET_CA")
AUTH=()
# GET <name> <url> [auth] — body to $OUT/<name>.json, response headers to $OUT/<name>.headers.
# No -L: a redirect to an SSO page is an answer here, not something to follow.
req() {
  local name=$1 url=$2 how
  local extra=()
  [[ ${3:-} == auth ]] && extra=("${AUTH[@]}")
  how=$(curl -sS ${TLS[@]+"${TLS[@]}"} ${extra[@]+"${extra[@]}"} -H 'Accept: application/json' \
    -o "$OUT/$name.json" -D "$OUT/$name.headers" -w '%{http_code} %{content_type} %{time_total}s' "$url" 2>"$OUT/$name.err")
  CODE=${how%% *}
  echo "  GET ${url#"$BASE"} → $how$( [[ -s $OUT/$name.err ]] && echo " ($(cat "$OUT/$name.err"))")"
}
json() { jq -r "$1" "$OUT/$2.json" 2>/dev/null; }

# --------------------------------------------------------------------- 2. TLS
step "2. TLS"
port=$(sed -nE 's#^https://[^/:]+:([0-9]+).*#\1#p' <<<"$BASE"); port=${port:-443}
tls_verify() { # $1 — the trust file, checked the way the app will check it
  local args
  args=(-CAfile "$1")
  [[ -s $inters ]] && args+=(-untrusted "$inters")
  openssl verify "${args[@]}" "$leaf" >/dev/null 2>&1
}
if [[ $BASE != https://* ]]; then
  note "TLS: none — the base URL is http"
elif ! command -v openssl >/dev/null; then
  note "TLS: not checked — no openssl here, and curl cannot answer this on macOS"
else
  echo "  chain the server sends (s: subject, i: issuer):"
  openssl s_client -connect "$BASE_HOST:$port" -servername "$BASE_HOST" -showcerts </dev/null 2>/dev/null \
    | grep -E '^ *[0-9]+ s:|^ +i:' | sed 's/^/    /'
  # Verified with `openssl verify`, never with `curl --cacert`: curl on macOS is
  # built with SecureTransport and consults the keychain, so it answers yes for
  # any certificate the user has installed system-wide — and "no PEM needed" is
  # then exactly the wrong answer for a corporate server.
  chain=$OUT/tls-chain.pem; leaf=$OUT/tls-leaf.pem; inters=$OUT/tls-inter.pem
  openssl s_client -connect "$BASE_HOST:$port" -servername "$BASE_HOST" -showcerts </dev/null 2>/dev/null \
    | awk '/BEGIN CERTIFICATE/,/END CERTIFICATE/' >"$chain"
  certs=$(grep -c 'BEGIN CERTIFICATE' "$chain")
  awk '/BEGIN CERTIFICATE/{n++} n==1' "$chain" >"$leaf"
  : >"$inters"
  i=2
  while [[ $i -le $certs ]]; do
    one=$OUT/tls-one.pem
    awk -v want="$i" '/BEGIN CERTIFICATE/{n++} n==want' "$chain" >"$one"
    if [[ -s $one ]]; then
      subj=$(openssl x509 -in "$one" -noout -subject | sed 's/^subject=//')
      issu=$(openssl x509 -in "$one" -noout -issuer | sed 's/^issuer=//')
      [[ $subj == "$issu" ]] || cat "$one" >>"$inters"  # a self-signed root is not an intermediate
    fi
    i=$((i+1))
  done
  if [[ ! -s $leaf ]]; then
    note "TLS: nothing answered on $BASE_HOST:$port"
  else
    if tls_verify /etc/ssl/cert.pem; then
      note "TLS: validates against the public roots alone — no PEM needed"
    else
      note "TLS: does not validate against the public roots — the server needs its CA as a PEM"
    fi
    if [[ -n ${BITBUCKET_CA:-} ]]; then
      if tls_verify "$BITBUCKET_CA"; then
        note "TLS: validates with BITBUCKET_CA alone — that PEM is the one for the form"
      else
        note "TLS: fails even with BITBUCKET_CA — wrong or incomplete PEM (the root, not the server's own cert?)"
      fi
    fi
  fi
fi

# ----------------------------------------------------- 3. Bitbucket, no token
step "3. is it Bitbucket — without a token"
req app-properties "$BASE/rest/api/latest/application-properties"
if [[ $CODE == 200 ]] && [[ $(json .displayName app-properties) == Bitbucket ]]; then
  note "anonymous application-properties: Bitbucket $(json .version app-properties) (build $(json .buildNumber app-properties))"
else
  note "anonymous application-properties: HTTP $CODE, not Bitbucket's JSON — auto-confirm is off, servers are added by hand"
  grep -i '^location:' "$OUT/app-properties.headers" | sed 's/^/    /'
fi

# --------------------------------------------------------------- 4. the token
step "4. the token"
TOKEN=${BITBUCKET_TOKEN:-}
[[ -z $TOKEN ]] && { read -rsp "  HTTP access token for $BASE: " TOKEN; echo; }
AUTH=(-H "Authorization: Bearer $TOKEN")

req repo "$API" auth
case $CODE in
  200) note "Bearer token: accepted, repository found" ;;
  401) note "Bearer token: 401 — not accepted (expired? HTTP access tokens off? SSO proxy in front?)"; exit 1 ;;
  404) note "Bearer token: 404 — repository $PROJECT/$SLUG not visible to it"; exit 1 ;;
  *)   note "Bearer token: HTTP $CODE"; exit 1 ;;
esac

# --------------------------------------------- 5. the repository is this one
step "5. clone links against origin"
norm() { sed -E 's#^[a-z+]+://##; s#^[^@/]*@##; s#\.git/?$##; s#/$##' <<<"$1" | tr '[:upper:]' '[:lower:]'; }
mine=$(norm "$REMOTE"); [[ $REMOTE != *://* ]] && mine=$(sed 's#:#/#' <<<"$mine")
matched=no
while read -r name href; do
  [[ -z $href ]] && continue
  echo "  $name $href"
  [[ $(norm "$href") == "$mine" ]] && matched=$name
done < <(json '.links.clone[]? | "\(.name) \(.href)"' repo)
note "origin matches clone link: $matched"

# ------------------------------------------------------------ 6. who am I
step "6. the current user's slug"
grep -iE '^x-a(username|userid):' "$OUT/repo.headers" | sed 's/^/    /'
HDR_USER=$(grep -i '^x-ausername:' "$OUT/repo.headers" | cut -d' ' -f2- | tr -d '\r')
req whoami "$BASE/plugins/servlet/applinks/whoami" auth
WHO=$(tr -d '\r\n' <"$OUT/whoami.json")
note "X-AUSERNAME header: ${HDR_USER:-absent}; applinks/whoami: HTTP $CODE ${WHO:0:60}"
NAME=${HDR_USER:-$WHO}
if [[ -n $NAME ]]; then
  req users "$BASE/rest/api/latest/users?filter=$(enc "$NAME")&limit=25" auth
  SLUG_ME=$(jq -r --arg n "$NAME" '.values[]? | select(.name == $n) | .slug' "$OUT/users.json")
  note "user: name=$NAME slug=${SLUG_ME:-not found} (approve uses the slug)"
fi

# --------------------------------------------------------- 7. default branch
step "7. default branch (Create PR's target)"
req default-branch "$API/default-branch" auth
db=$(json .displayId default-branch); where=/default-branch
if [[ $CODE != 200 ]]; then
  req default-branch-old "$API/branches/default" auth; db=$(json .displayId default-branch-old); where="only /branches/default"
fi
note "default branch endpoint: $where → ${db:-?}"

# ------------------------------------------------------ 8. branch on server
step "8. is $BRANCH on the server"
req branches "$API/branches?filterText=$(enc "$BRANCH")&limit=25" auth
BR_SHA=$(jq -r --arg b "$BRANCH" '.values[]? | select(.displayId == $b) | .latestCommit' "$OUT/branches.json")
note "branch $BRANCH on server: ${BR_SHA:-no}"

# --------------------------------------------------------------- 9. the PR
step "9. the PR of $BRANCH"
req prs "$API/pull-requests?at=$(enc "refs/heads/$BRANCH")&direction=OUTGOING&state=OPEN" auth
PR=$(json '.values[0].id // empty' prs)
if [[ -z $PR ]]; then
  note "open PR for $BRANCH: none"
  req prs-any "$API/pull-requests?state=OPEN&limit=5" auth
  echo "  branches with an open PR here — run again with one of them for steps 10–11:"
  json '.values[]? | "    \(.fromRef.displayId)  #\(.id) \(.title)"' prs-any
else
  note "open PR: #$PR version=$(json .values[0].version prs) author=$(json .values[0].author.user.slug prs) → $(json .values[0].toRef.displayId prs)"
  echo "  reviewers:"; json '.values[0].reviewers[]? | "    \(.user.slug) \(.status)"' prs
  note "PR link: $(json '.values[0].links.self[0].href' prs)"

  step "10. can it be merged"
  req merge "$API/pull-requests/$PR/merge" auth
  note "merge check: canMerge=$(json .canMerge merge) conflicted=$(json .conflicted merge) outcome=$(json '.outcome // "-"' merge)"
  json '.vetoes[]? | "    veto: \(.summaryMessage)"' merge
fi

# ----------------------------------------------------------------- 11. builds
step "11. builds of the head commit"
SHA=$(json '.values[0].fromRef.latestCommit // empty' prs); SHA=${SHA:-$BR_SHA}
if [[ -z $SHA ]]; then
  note "builds: no commit on the server to ask about"
else
  echo "  commit $SHA"
  req builds "$API/commits/$SHA/builds" auth;                new=$CODE
  req builds-legacy "$BASE/rest/build-status/latest/commits/$SHA" auth; old=$CODE
  note "builds: repo endpoint HTTP $new with $(json '.values | length' builds) item(s); legacy HTTP $old with $(json '.values | length' builds-legacy) item(s)"
  json '.values[]? | "    \(.state) \(.key) \(.name // "") \(.url)"' builds
  [[ $new != 200 ]] && json '.values[]? | "    legacy: \(.state) \(.key) \(.name // "") \(.url)"' builds-legacy
fi

# ---------------------------------------------------------- 12. token rights
step "12. the token's permissions"
if [[ -n ${SLUG_ME:-} ]]; then
  req tokens "$BASE/rest/access-tokens/latest/users/$SLUG_ME" auth
  [[ $CODE == 200 ]] && json '.values[]? | "    \(.name): \(.permissions | join(",")) expires \(.expiryDate // "never")"' tokens
  note "token list: HTTP $CODE (merge and create need REPO_WRITE; read and approve need REPO_READ)"
fi
