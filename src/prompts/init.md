Your task is to study this repository and prepare two files in its root:

- **AGENTS.md** — the main, tool-agnostic guide for any AI coding agent (Kibo, Codex, Cursor, Copilot, Claude Code and others): project facts plus engineering principles.
- **CLAUDE.md** — a thin wrapper for Claude Code, which reads CLAUDE.md rather than AGENTS.md. It imports AGENTS.md and adds only Claude-specific notes.

Both files are loaded into every future session, so every line must earn its place.

Additional focus from the user (may be empty): $ARGUMENTS

## Step 1. Explore the project

Read, where they exist:

- the README and other docs in the root and in `docs/` (skim long ones; you need facts, not every page)
- manifests and build configs: `package.json`, `pyproject.toml`, `requirements*.txt`, `Cargo.toml`, `go.mod`, `pom.xml`, `build.gradle`, `Makefile`, `Justfile`, `Dockerfile`, `docker-compose.*`
- linter, formatter and test configs (eslint, prettier, ruff, mypy, jest, pytest, etc.)
- CI pipelines: `.github/workflows/`, `.gitlab-ci.yml` and similar
- rules for AI agents: `AGENTS.md`, `CLAUDE.md`, `.cursorrules`, `.cursor/rules/`, `.github/copilot-instructions.md`

Then look at the layout — the tracked files (`git ls-files`) rather than the whole tree — and open a few key files to understand the architecture, layering and code style. Recent commits (`git log --oneline -20`) show the commit message conventions.

While exploring, determine specifically:

- which architectural style the project actually uses (layered, hexagonal/ports-and-adapters, clean architecture, feature-sliced, MVC, plain modules…) and where the boundaries are;
- which direction dependencies flow between modules;
- where business logic lives and where I/O (database, HTTP, filesystem, external APIs) lives.

## Step 2. Write AGENTS.md

Section A comes from the project; section B is the engineering principles block, adapted to it.

### A. Project-specific — only what the files confirm

1. **Overview** — one or two sentences: what the project is and its main stack.
2. **Commands** — build, run, lint, format, type-check, test, and especially how to run a *single* test. List a command only if a manifest, script or CI config shows it exists.
3. **Architecture** — the big picture no single file shows: the main modules or layers, how they relate, which dependency directions are allowed, where things go ("new endpoints go in X, business rules in Y, database access only in Z").
4. **Conventions** — code style, naming, file layout, commit messages — only where they differ from common defaults or are enforced by tooling.
5. **Gotchas** — important, non-obvious details from the README, CI and other agents' rules.

### B. Engineering principles

Include the block below, adapted to the project rather than copied:

- if the real architecture is not generic layering (hexagonal, feature-sliced, …), rewrite "Architecture & layering" in terms of the actual layers and folder names you found;
- where a principle clearly conflicts with an established project convention, follow the project and drop or adjust the principle;
- remove what does not apply to this stack (UI rules for a CLI tool, say);
- keep it short — rules, not essays.

```markdown
## Engineering principles

### Simplicity
- **KISS**: prefer the simplest solution that works. Plain functions and data over clever
  abstractions. If a junior developer can't follow it quickly, simplify it.
- **YAGNI**: implement only what the current task requires. No speculative options, hooks,
  config flags, or generic frameworks "for the future".
- **DRY, but with the rule of three**: extract shared code when the same logic appears the
  third time and the copies change for the same reason. Duplication is cheaper than the
  wrong abstraction.
- Prefer composition over inheritance. Keep inheritance hierarchies shallow.

### Architecture & layering
- Respect the layers: presentation (API/UI/CLI) → application (use cases, orchestration)
  → domain (business rules, entities) ← infrastructure (DB, HTTP clients, queues, files).
- Dependencies point inward: the domain does not import from infrastructure or
  presentation. Infrastructure implements interfaces the inner layers define.
- No business logic in controllers/handlers/views or in data-access code.
- Don't skip layers (e.g. a handler querying the DB directly) unless the project already
  does so deliberately for that case.
- One module, one responsibility. Keep modules cohesive and loosely coupled.
- Depend on abstractions at boundaries, but don't introduce an interface that has only one
  implementation and no testing need.

### Code quality
- Small, focused functions with clear names. Names describe intent, not implementation.
- Make illegal states unrepresentable where the language allows (types, enums, value objects).
- Prefer pure functions and immutable data; isolate side effects at the edges.
- Validate input at system boundaries; trust data inside the core after validation.
- Fail fast with explicit, informative errors. Never swallow errors silently.
- No dead code, commented-out code, or stray debug output.
- Comments explain *why*, not *what*.

### Dependencies & security
- Don't add a dependency if the standard library or an existing dependency covers it.
  Justify every new one.
- Never commit secrets, tokens or credentials. Use configuration or environment variables.
- Treat all external input as untrusted (injection, path traversal, deserialization).

### Testing
- New behavior comes with tests; a bug fix comes with a regression test.
- Test behavior through public interfaces, not implementation details.
- Unit-test the domain without infrastructure; keep integration tests at the boundaries.
- Tests must be deterministic: no reliance on real time, network or ordering.

### Working style for agents
- Make the smallest change that solves the task. Don't refactor, reformat or rename
  unrelated code in the same change.
- Follow existing patterns in the codebase before inventing new ones.
- Read the surrounding code before editing it.
- Before finishing: run the linter, type checker and relevant tests; fix what you broke.
- If requirements are ambiguous or a change would break an architectural rule, ask
  instead of guessing.
- Don't leave TODOs without explaining them in your final summary.
- Edit AGENTS.md when a command, convention or layer it describes changes — never to
  record progress.
```

Rules for AGENTS.md as a whole:

- don't invent commands, practices or sections the project doesn't have (section A);
- don't list every file or state the obvious;
- only what stays true until a convention changes: no progress, current status, dates or "so far" / "for now" — what is built and what comes next belongs in the project's docs or plans, not here. Every agent rereads this file on every request, and an edit to it makes the conversation that follows start over;
- keep it under about 200 lines — a small project needs far fewer;
- write in the language of the repository's existing agent instructions, or in English if there are none.

## Step 3. Write CLAUDE.md

Keep it minimal:

```markdown
# CLAUDE.md

This file provides guidance to Claude Code when working with code in this repository.

@AGENTS.md
```

Below the import, add only what is specific to Claude Code (hooks, slash commands or subagents this repository uses). If there is nothing Claude-specific, leave just the import. Do not repeat anything from AGENTS.md — agents that read both files would get it twice.

## Step 4. If the files already exist

Do not overwrite them.

- **AGENTS.md exists**: compare it with what you found and propose concrete edits — what to add (missing principles included), what is outdated, what is redundant.
- **CLAUDE.md exists with real content**: propose moving its tool-agnostic parts into AGENTS.md and replacing them with the `@AGENTS.md` import, keeping only Claude-specific notes in CLAUDE.md.

Show the proposed changes as diffs, then end your turn and wait. Apply them only after the user confirms, and only the ones they confirmed.

## Step 5. Summary

Briefly list what went into each file, which principles you adapted or dropped and why, and what the user should add by hand — things the code cannot tell you: team agreements, prohibitions, business context.
