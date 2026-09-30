# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Architecture, layering rules, IPC conventions, error handling and UI rules live in
[`AGENTS.md`](./AGENTS.md) — read it, it is the primary source:

@AGENTS.md

What follows is only what `AGENTS.md` does not cover.

## Tests

Frontend tests run on **Bun's own runner** (not vitest/jest), from `src/__tests__/`:

```bash
bun test                                  # all frontend tests
bun test src/__tests__/Dropdown.test.tsx  # one file
bun test -t "Escape"                      # one test by name pattern
```

`bunfig.toml` preloads `src/__tests__/setup/happydom.ts`, which registers the DOM
globals and auto-unmounts after each test. A test that renders components works
only through `bun test` — running the file with `bun run` gives it no DOM.
`tsconfig.json` excludes `src/__tests__/**`: Bun type-checks those itself, and `tsc`
does not know `bun:test`.

Rust tests are inline `#[cfg(test)] mod tests` blocks, next to the code they cover:

```bash
cd src-tauri && cargo test              # all
cd src-tauri && cargo test resolve      # by name substring
```

Tests that load the bundled embedding model (`infra/local_embeddings.rs`) are
`#[ignore]`d: they cost seconds and most of a gigabyte per run, which a mutation run
pays once per mutant. Run them after touching the model, its loading, or embedding:

```bash
cd src-tauri && cargo test local_embeddings -- --ignored
```

The default run still checks that the weights are real files and not Git LFS pointers.

Search quality has its own bench, ignored by default because it indexes real
repositories (`src-tauri/bench/search-queries.json` lists them and the questions).
Run it after touching ranking in `services/code_search.rs`:

```bash
cd src-tauri && cargo test --release search_bench -- --ignored --nocapture
```

The agent as a whole has one too: small broken repositories in `src-tauri/bench/agent-tasks/`,
a real model working on each, a hidden check of the result, and pass rate, rounds, tool
errors and tokens per run. Run it after touching the loop, the prompt or a tool, with
the same model as before — the provider comes from `AGENT_BENCH_*` variables, listed
at the top of `services/agent_bench.rs`:

```bash
cd src-tauri && AGENT_BENCH_API_KEY=… AGENT_BENCH_MODEL=… AGENT_BENCH_RUNS=3 cargo test --release agent_bench -- --ignored --nocapture
```

The Kubernetes role has one as well: a namespace with something wrong in it on the local
cluster (OrbStack), a request, and a hidden check of the cluster and the answer —
`src-tauri/bench/kube-tasks/`. The provider is the agent bench's (`AGENT_BENCH_*`). Run it
after touching the role's prompt, a `kube*` tool or a runbook:

```bash
cd src-tauri && AGENT_BENCH_API_KEY=… AGENT_BENCH_MODEL=… AGENT_BENCH_RUNS=3 cargo test --release kube_bench -- --ignored --nocapture --test-threads=1
```

Its tasks check themselves without a model — untouched fails, the reference solution
passes, the wrong one fails. Run that after adding or changing a task:

```bash
cd src-tauri && cargo test kube_fixtures -- --ignored --nocapture
```

The Kubernetes client has a live test against the local cluster (OrbStack's, context
`orbstack` in `~/.kube/config`), ignored because it needs that cluster running. Run it
after touching `infra/kube_client.rs`:

```bash
cd src-tauri && cargo test live_cluster -- --ignored --nocapture
```

## Mutation testing

New backend code is not done when `cargo test` is green — the tests have to be shown to
catch something. Break your own logic one edit at a time and check that each break fails
a test; a survivor is a missing test, not a bad mutant.

The method, the outcomes and what to write in the report are in
[`docs/11-mutation-testing.md`](docs/11-mutation-testing.md). The runner takes a JSON plan
(there is no `cargo-mutants` here):

```bash
python3 scripts/mutate.py plan.json -k
```

## Checks before done

```bash
bun run tsc --noEmit && bun test
```

```bash
cd src-tauri && cargo check && cargo test
```

## Dev server

`bun run tauri dev` runs the full app. The Vite dev server alone is registered in
`.claude/launch.json` as `vite-dev` on port 1420 — useful for previewing UI, but
every `invoke()` fails there because there is no Tauri backend.

Port 1420 is `strictPort` — if it is taken, the dev server exits instead of
picking another, and `tauri dev` then fails with a blank window.

## Build notes

- `scripts/clean-build-cache.sh` reclaims Rust target space.
- `scripts/embedding-model.md` describes the bundled local embedding model: its cost,
  how to rebuild it with `scripts/build-embedding-model.py`, and what to check after.
  The weights are Git LFS objects — `git lfs pull` after a clone, or the model tests
  fail with a message saying so.
