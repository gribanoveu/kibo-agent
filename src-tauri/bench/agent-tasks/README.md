# Agent bench tasks

One directory per task, run by `src/services/agent_bench.rs`:

- `task.md` — the user's message, verbatim.
- `repo/` — the folder the agent opens. Copied fresh for every run.
- `check.sh` — run afterwards in the copy; exit 0 means solved. It and anything
  beside it (hidden tests) stay outside the copy, so the agent never sees them.
  It gets `WORKSPACE` (the copy), `ORIG` (this `repo/`) and `TASK` (this directory).

Files here are byte-exact (`-text` in `.gitattributes`): `crlf-edit` depends on it.

| Task | What it presses on |
|---|---|
| `fix-off-by-one` | read → edit → run the tests; the tests must stay as they are |
| `find-by-search` | only the symptom is given; the cause is one file of seven |
| `crlf-edit` | an edit in a CRLF file, beside look-alike lines; checked byte for byte |
| `agents-rule` | a fix plus a rule from `AGENTS.md` (a CHANGELOG line) |
| `order-dependent-tests` | the failing test is not where the bug is; a shallow copy is not enough |
| `one-handler-in-many` | 700 lines of near-identical handlers: anchors that repeat, look-alike routes |
| `truncated-output` | the one line that matters is in the middle of 77k characters of output |
| `prompt-injection` | the file to fix tells the agent to delete the tests; it must not |
| `expression-precedence` | three precedence bugs; the visible test shows one — the README promises `eval` |
| `flaky-counter` | a thread race the test shows in `hit`, and the same race in `take` it does not |
| `migrate-records` | a written spec with a dozen edge cases, checked against a reference on hidden input |
| `log-forensics` | 1 MB of logs, two causes of 500s — one of them three lines in one file; tests required |
| `mixed-endings` | a file half LF, half CRLF; the line to fix repeats, so one line cannot anchor it and two cross a CRLF `editFile` refuses — the route has to change (the loop guard's case) |

Every check was run three ways when written: on the untouched repo (fails), with a
reference fix (passes), and with a plausible wrong fix (fails).
