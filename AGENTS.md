# AGENTS.md

Context for AI coding agents (Claude Code and others) working in this repository.

## Project

**Kibo Agent** - a cli agent like claude code desktop 

- Identifier: `com.kibo.agent`
- Stack: Tauri v2, React + TypeScript frontend, Rust backend
- Package manager: **bun** — always use `bun`/`bunx`, never `npm`/`pnpm`/`yarn`

Tech stack
Frontend: React 19 + TypeScript, Vite build, plain CSS files per component (no CSS-in-JS/Tailwind), lucide-react for icons, streamdown + shiki for streamed Markdown and highlighting (`components/Markdown.tsx`). No global state library (Redux/Zustand/Context store) — state lives in custom hooks (`src/hooks/*`, ~20 of them) composed directly into components (e.g. `useAgentTurn`, `useWorkspace`).

`App.tsx` is the composition root: it calls most of those hooks and passes their results down as props. That is where cross-cutting state actually lives — adding a hook that more than one panel needs usually means editing it, and the file grows with every one. Before adding state there, check whether the hook can own it privately, or whether an existing hook already exposes it. React Context is not used at all; keep it that way unless a subtree genuinely needs it.

Backend: Tauri v2 (Rust), ureq (blocking HTTP client, not reqwest) for LLM provider calls, tauri::async_runtime::spawn_blocking to run them off the async runtime. Streaming deltas and other progress reach the frontend as tauri::Emitter events, emitted in commands/ only — services report through sinks (see Architecture).

## Setup & commands

```bash
bun install                 # install frontend deps
bun run tauri dev           # run the app with hot reload
bun run tauri build         # production bundle
bun run tsc --noEmit        # type-check frontend only
cd src-tauri && cargo check # fast Rust check while iterating
cd src-tauri && cargo test  # run Rust tests
cd src-tauri && cargo add <crate>   # add a Rust dependency
bun add <pkg>                       # add a JS dependency
```

Run `bun run tsc --noEmit` and `cargo check` before considering a change done. If tests exist for the touched area, run them too — don't assume green.

## Architecture

The app is layered; keep new code in the right layer instead of adding logic to the boundary.

```
src/                      # React frontend
├── components/           # render only — no fetching, no business rules
├── hooks/                 # frontend application layer: call invoke() wrappers, hold state
└── lib/                    # two things: one typed wrapper per Tauri command, plus shared pure helpers

src-tauri/src/
├── commands/              # boundary: thin #[tauri::command] fns — validate input, call a service, map errors to String
├── services/               # application layer: use-case orchestration
├── domain/                  # pure types + business rules + typed error enums (thiserror), no I/O, no framework types
└── infra/                    # git2 / filesystem / network — concrete implementations of domain traits
```

Dependency direction points inward: `commands → services → domain`, and `infra` implements traits that `domain`/`services` define — never the reverse. Don't reach for `git2` or `tauri::` types from inside `domain/` or `services/`.

**Reporting outward crosses a port, never an `AppHandle`.** A service that needs to tell the UI something takes a sink — `Arc<dyn Fn(Event) + Send + Sync>`, with the event type in `domain/` — and the command layer is the only place that turns those into Tauri events. The one that exists here today is `domain::turn::ChatEventSink`, adapted in `commands/chat_events.rs`; `domain::command_exec::CommandSink` is the same shape one layer down. The index layer's is `domain::workspace_index::IndexEventSink` (one `IndexEvent` enum: sync started, keywords ready, embedding progress, sync finished, failed), taken by `services::index_sync::RepoIndexer::sync` and `services::workspace_index::WorkspaceIndex::open`, adapted in `commands/workspace_events.rs`. The background processes' is `domain::background::ProcessEventSink` (one `ProcessChanged { id }` signal), taken by `infra::background::Processes::new`, adapted in `commands/processes.rs`. The helper agents' is `domain::agents::AgentEventSink` (one `AgentChanged { id }` signal), taken by `domain::agents::Agents::new`, adapted in `commands/agents.rs`. When a service reports more than one kind of thing, use one enum rather than several callbacks. `tauri::async_runtime` used purely as a thread pool (`spawn_blocking`) is fine in `services/` — that's a runtime, not the UI.

Don't pre-build all four layers for something trivial. Introduce a trait boundary when there's a real second implementation (e.g. a test double) or a use-case spanning multiple infra calls — not speculatively.

The AI-agent tool surface lives in `domain/tools.rs` (tool identity, loop cost, the approval gate) and `services/ai_tools/` (one module per tool under `tools/`, plus path resolution, argument parsing and previews). It is fully wired: `services::llm_chat` runs the tool-calling loop against it, and the chat panel drives it from the UI. Adding a tool is a variant in the two enums, a module under `services/ai_tools/tools/`, and a branch in the `match` — if a fifth place needs editing, that contract is broken.

## Errors

- Model failures as data as deep into the stack as possible: `thiserror` enums in `domain/`, not `String`.
- Flatten to `String` (or a small serializable DTO) only inside `commands/`, at the IPC boundary — that's the one place stringly-typed errors are acceptable.
- No `unwrap()`/`expect()` outside of tests and truly-unreachable invariants.

## IPC conventions

- Every `#[tauri::command]` gets a matching typed wrapper in `src/lib/` — components/hooks call the wrapper, never `invoke()` directly.
- New commands must be registered in `generate_handler![]` in `lib.rs` (`main.rs` only calls `run()`), and any new plugin/API surface needs a corresponding entry in `src-tauri/capabilities/*.json` — a command that "does nothing" at runtime usually means a missing capability entry, not a missing registration.
- Long-running git operations (clone, fetch) run as `async` commands or via `spawn_blocking`, not on the IPC event loop.

### Keeping the frontend current

The frontend does not poll the backend for state. When backend state changes, the backend says so on a Tauri event, and the frontend reads again — a timer that re-asks is a bug, not a fallback.

- **Tauri's events are the bus.** There is no app-wide bus or dispatcher on top of them: a named channel per source, with one enum per channel (the sink rule above), is what a generic bus would reduce to, minus the types. Today's channels: `chat:turn-event` (`commands/chat_events.rs`), `workspace-index:event` (`commands/workspace_events.rs`), `workspace-git:changed` (`commands/git.rs`), `processes:changed` (`commands/processes.rs`), `agents:changed` (`commands/agents.rs`), `terminals:changed` (`commands/terminal.rs`). The name is a `pub const` beside the emitter, and a test pins it — the frontend's copy in `src/lib/` is a string.
- **An event names what it is about** — the folder (`root`, the string `workspace_open` returned) or the turn (`turnId`). Events outlive their subject: a sync of the folder just left still reports, and a listener that does not check would apply it to the next one.
- **An event is a signal to re-read, not the new state**, unless the payload is the data itself (a streamed delta). The listener calls the command it already has. This keeps one source of truth, and a lost or coalesced event costs nothing: the next one re-reads everything.
- **A byte stream goes down a `tauri::ipc::Channel`, not an event.** A terminal's output is the data itself and only the tab drawing that terminal wants it, so `terminal_attach` takes a `Channel` and sends raw bytes (they arrive as an `ArrayBuffer`); the tab detaches it on cleanup. `terminals:changed` still says the list changed.
- **Reuse a signal before adding one.** A change to the working tree already arrives as the index's `syncStarted`; `.git` is the one part that watcher leaves out, which is why `workspace-git:changed` exists. A new channel is for a change no existing one reports.
- **Listen only while it matters.** A hook subscribes while its pane is on screen and unsubscribes on cleanup (`useStaging` is the pattern), reading once when it starts listening — what changed while it was hidden is not replayed.
- **A source that changes fast is throttled where it changes**, not in the listener. A background process writing a line per millisecond is signalled at most once per 100 ms tick (`infra/background.rs`, `watch`), so the IPC channel never carries a burst the frontend then has to debounce.
- Wrappers in `src/lib/` own the channel: `onIndexEvent`, `onGitChanged`, `onProcessChanged`, `onAgentChanged`, `onTurnEvent` — components and hooks never call `listen()` directly, as with `invoke()`.

A clock that only redraws elapsed time (`ChatPanel`) is not polling.

## Filesystem & git

- Prefer `git2` over shelling out to `git` for programmatic operations (diff, blame, branch listing); shell out only for actions you don't want to reimplement, isolated to one module.
- Validate/canonicalize any path coming from the frontend against the opened repo root before touching it.
- Use `std::path::Path`/`PathBuf` throughout — no manual path string concatenation (the app targets macOS/Windows/Linux).

## Style

- Prefer immutable data and explicit, exhaustive error handling (`match` over ignoring `Err`).
- Keep abstractions proportionate to actual need — don't add a layer, trait, or generic parameter for a hypothetical future case.
- Match existing patterns in the file/module you're editing before introducing a new one.

### UI

**Don't use a browser control where the app already draws its own.** The app renders its interactive widgets itself, so a native one arrives with the platform's look — a macOS `<select>` among hand-styled panels reads as something pasted in from another program, and it ignores the theme tokens everything else is built from.

The pattern for a dropdown is a `<button>` trigger plus a `role="listbox"` menu of `role="option"` buttons, dismissed by an outside `pointerdown` or `Escape`. It is written once, in `src/components/Dropdown.tsx` — use that component instead of a second implementation (`Composer`, `Sidebar`, `ChatMenu` and `ToolLog` all do). The same applies to anything else the platform would draw its own way: dialogs go through `src/components/Modal.tsx`, never `alert`/`confirm`.

**A key the app answers to is an entry in `src/lib/shortcuts.ts`.** A window-wide one is handled through `useShortcuts({ id: handler })`; a key local to one component checks `matches(e, id)` instead of spelling out `e.key`. The shortcuts dialog and the menus' hints are drawn from that registry, so a key written anywhere else is one the user is never told about.

Colours, spacing and fonts come from the tokens in `src/styles/tokens.css` (`--bg-*`, `--text-*`, `--border`, `--accent`, `--font-ui*`). A literal hex or pixel font size in a component is a bug: it will not follow the user's theme or font-size preference.

A component used from more than one place carries its own styles rather than borrowing a neighbour's — CSS is bundled globally, so borrowing appears to work right up until the neighbour is deleted.

## Commit / PR expectations

- Keep commits scoped to one layer or one concern where practical.
- Run the relevant checks above before marking work done; mention in the summary which checks were run.
- A finished feature adds one list item to `CHANGELOG.md` at the repo root, under `## Unreleased`, newest on top: the date and what the user can now do, e.g. `- 2026-09-26 Add selector for thinking level for providers`. Create the file if it is missing, with a `# Changelog` title and an empty `## Unreleased`. Refactors, tests and internal fixes the user never sees don't get a line.
- Creating a release tag closes the section: rename `## Unreleased` to `## <tag> — <date>` (e.g. `## v0.1.2-alfa — 2026-09-27`), open a new empty `## Unreleased` above it, and commit that before tagging, so the tag holds its own changelog.
