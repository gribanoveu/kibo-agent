# Changelog

## Unreleased

- 2026-10-01 A turn that runs into its limits keeps its work: "continue" goes on from where it stopped instead of starting the task over
- 2026-10-01 Set how many rounds and how much tool budget one agent turn may spend, in Settings → Agent
- 2026-10-01 In the / menu, Enter puts a command that takes arguments in the box for you to type them, instead of running it without; a line under the menu says what Enter and Tab do with the highlighted command
- 2026-10-01 Typing a / command shows the arguments it still wants in grey after the text, `/compare <old> <new> [focus]`, each one going as you type it
- 2026-10-01 Your own / commands take arguments one by one, as in Claude Code: `$0`, `$1` or `$ARGUMENTS[N]` for each (quotes keep words together), names from `arguments` in the frontmatter as `$name`, and `\$1` for a literal `$1`; what no placeholder takes arrives at the end as `ARGUMENTS: …`
- 2026-10-01 Switch single MCP tools off in the MCP tab, and set a server's tools to be found with `toolSearch` instead of declared in every request (`"exposure": "deferred"`, per tool with `toolExposure` and `*` patterns); a very large MCP result reaches the model with its middle cut and is saved whole for it to read
- 2026-09-30 See the provider's output speed in tokens per second beside Working, live while it streams, and the session's average beside Worked for

## v0.3.1-alfa — 2026-09-30

- 2026-09-30 Settings → Web search shows how many Tavily credits the key has used this billing cycle and how many are left, and says when Tavily refuses the key; without a key, a chat's model says it cannot search and where to add one, instead of writing out a search call as text

## v0.3.0-alfa — 2026-09-30

- 2026-09-30 In Chat mode the model can search the web with a Tavily key, added in Settings → Web search; each search shows in the chat

- 2026-09-30 An MCP server can ask you something in the middle of a call — a form or a page to open — and the call goes on with your answer

- 2026-09-30 The prompts of running MCP servers are commands in the / menu, as /server:prompt

- 2026-09-30 A tool its MCP server marks destructive asks before every call, even under Always allow; the MCP tab shows what each server says about its tools

- 2026-09-30 An MCP server's instructions reach the model, and the MCP tab shows what the server tells it

- 2026-09-30 An MCP server that changes its tools offers the new ones from the next message, without a restart

- 2026-09-30 MCP servers that speak the 2026-07-28 revision of the protocol connect; older ones work as before

- 2026-09-30 The Skills panel lists the skills that are on first and the ones switched off after them, in grey

- 2026-09-30 Settings → Skills: a folder opens under its row to show the skills it gave, with the hidden and switched-off ones marked

- 2026-09-30 The Kubernetes role no longer sends a password it reads in a ConfigMap or a log line to the model — the value after a key named like a credential, and the password in a URL, are hidden; a change that destroys data for good — deleting a claim whose volume goes with it, scaling down or deleting a StatefulSet that deletes its claims — is refused until you have told the chat that losing it is acceptable

- 2026-09-30 Settings → Kubernetes is three tabs — Kubeconfigs, Runbooks, Changes — and a runbook opens under its row to be read

- 2026-09-30 The chat list scrolls by itself, so the account row and the first-steps card stay on screen however many chats there are; a Chat mode conversation wears its role's sign in the list

- 2026-09-30 The Kubernetes role follows runbooks: seven built in — crash loops, image pulls, Pending, OOMKilled, failing probes, requests that do not arrive, and a Spring Boot application — and your own from `~/.kibo/runbooks/kubernetes`, which replace a built-in one of the same name; Settings → Kubernetes lists them

- 2026-09-30 The Kubernetes role checks from inside a pod whether it reaches a URL, a port or a name — resolved, open, the HTTP status, refused or timed out — without reading the response and without running anything else in the pod

- 2026-09-30 The Kubernetes role waits for a rollout instead of asking again and again: after a restart, a rollback, an apply or a scale it says when the pods are up — or that the rollout is stuck or not done in time, and why

- 2026-09-30 Mark a kubeconfig as production in Settings → Kubernetes: every change to its clusters then asks even for a tool you always allow, and the card says PRODUCTION first; a change's card also names what will put it back by itself — an autoscaler, Argo CD or Flux, Helm, an owner

- 2026-09-30 The Kubernetes role can apply a manifest and delete an object in the chat's namespace: the card shows each object's diff before you agree, a deleted PersistentVolumeClaim says when its data goes with it, and undo takes away what an apply created, restores what it changed and recreates what was deleted

- 2026-09-30 The Kubernetes role can suspend and resume a CronJob or Job, restart a Deployment, StatefulSet or DaemonSet, and roll a Deployment back to an earlier revision — each on a card that shows what changes (for a rollback, the revisions and images) and says when it cannot be undone

- 2026-09-30 Undo a change Kibo made to a cluster: ask the Kubernetes chat to undo it and the object is put back from its backup, for 30 days, on a card you approve — never over what someone else changed since; Settings → Kubernetes → Changes lists the changes and their ids

- 2026-09-30 Let a Kubernetes chat change the cluster: switch its tab from Read only to Changes and the model can scale a Deployment, StatefulSet or ReplicaSet in the chat's namespace — each change asks on a card that names the cluster and shows the replicas before and after, and the object is backed up first

- 2026-09-29 Ask the Kubernetes role why something is failing and it diagnoses it in one step: the workload's status, what is wrong with each pod, the events of it and its ReplicaSets, and the log of the crash from before its last restart — or, for a pending pod, what it asked for and which volume claim waits

- 2026-09-29 A chat keeps the role it started with: the role is picked before the first message and fixed after it
- 2026-09-29 Chat mode shows the context meter beside the send button, as the agent does: how full the model's window is, what it is spent on, what the last request cost, and "Compact now" — and folds the older part of a long chat on its own before the next message
- 2026-09-29 The Kubernetes role reads your cluster itself: lists as `kubectl get` tables (with any fields as extra columns), objects as YAML without the noise, events, logs of a pod or of every pod of a Deployment merged by time, CPU and memory, and who set each field — only in the chat's pinned cluster, never changing it, and never seeing a Secret's values

- 2026-09-29 Pin each Kubernetes chat to a kubeconfig, a context and a namespace on its tab — namespaces come from the kubeconfig, from the cluster when you may list them, or typed; the model is told the cluster's version, what your kubeconfig may do there, or that the cluster did not answer and why
- 2026-09-29 Chat mode draws its conversation as the agent's chat does — turns under the role's name, thinking rows, Copy — and a role's tool calls, with their approval cards, will show there; chats saved before still open
- 2026-09-28 Copy a message or an answer in Chat mode with the Copy button under it, shown on hover as in the agent's chat
- 2026-09-28 Add your kubeconfig files in Settings → Kubernetes and pick one on the Kubernetes role's tab in Chat mode: the commands the model gives name that file; with none set up, it says where to add one
- 2026-09-28 Chat mode's empty chat shows the role's icon in a thought cloud over Kibo, and the role's tab carries the same icon
- 2026-09-28 Pick the Kubernetes role in Chat mode: the model answers as a Kubernetes engineer — manifests, Helm, why a pod fails — asks for the kubectl output it needs and marks commands that change the cluster
- 2026-09-28 Scroll up to read while an answer is still arriving: the thread no longer drags you back to the end, in the agent and in Chat mode; scrolling back down follows it again
- 2026-09-28 Save an API key without a keychain prompt: keys are kept in ~/.kibo, readable only by your account, and Settings → Provider can move them into the system keychain and back
- 2026-09-28 Stop a request while the model has not answered yet: the Stop button now works within a moment in both the agent and Chat mode, even before the first word arrives
- 2026-09-28 An MCP server that refuses with HTTP 405 now says why: an address ending in /sse is the old SSE transport, and names the Streamable HTTP address to use instead
- 2026-09-28 Add a Chat mode, switched in the sidebar: a plain conversation with the model, with no folder, files or tools; its chats are saved and listed in the sidebar, the role is picked on a tab above its message box, the model and the thinking level in the box; the model's thinking is shown folded above each answer

## v0.2.0-alfa — 2026-09-28

- 2026-09-27 /review has an ordinary turn's budget, 60 rounds, and is asked to wrap up with a fifth of it left, not after 10 rounds
- 2026-09-27 Helpers the agent asks for one after another in a reply run at the same time, up to four at once
- 2026-09-27 An explore row in the chat, unfolded, has Show in Agents to open that run in the Agents tab; / commands such as /init now also ask for replies in the language chosen in Settings
- 2026-09-27 With a reply language chosen in Settings, a long /review no longer drifts into another language: the app's own notes to the agent repeat it
- 2026-09-27 See every helper agent in Terminal → Agents: its question, steps, tokens spent and answer, with Stop for one still running — the agent carries on without it; the explore card shows the tokens too
- 2026-09-27 The agent can hand a research question to a read-only helper (`explore`): the helper searches and reads in a context of its own, its steps show on the call's card, and only its answer comes back into the chat
- 2026-09-27 A new chat shows up in the sidebar as soon as its first message or /review is sent, not when the agent finishes
- 2026-09-27 Pick the language the agent answers in — Auto, English or Russian — in Settings → Language; it covers chat answers, plans and /review
- 2026-09-27 A chat with a turn under way is no longer left behind unsaved by New chat or another chat: it says to stop the turn first
- 2026-09-27 Type /review to have the agent review your uncommitted changes: it reads the code it needs and runs checks, but changes no files; each finding shows as a card on the line it is about, with Fix to have the agent fix it, and a review that runs long is asked to wrap up
- 2026-09-27 Rewind a chat to before one of your messages: the files the agent changed since go back to how they were — never over a change made after it — and the message returns to the box to change and send again
- 2026-09-27 See how many background processes are still running beside "Working…" and "Worked for"; a click opens the newest in the Terminal tab
- 2026-09-27 A command the agent runs is no longer killed at its timeout: it goes on as a background process, shown in the Terminal tab, and the agent reads it from there instead of running it again
- 2026-09-26 The agent ends a reply with a concrete question offering the next step when there is an obvious one; each finished turn and the message you sent after it are kept locally in chats.db, to train the upcoming next-message suggestions
- 2026-09-26 Press ⌥Enter (Alt+Enter) while the agent works to queue a message as the next turn; it is sent once the turn finishes, and a stopped or failed turn puts it back in the message box
- 2026-09-26 An agent that repeats the same call to the same result, or fails the same way three rounds running, is told once to change course, and the chat shows that it was
- 2026-09-26 Hear a sound when the agent finishes or fails, and get a system notification when Kibo is in the background — finished, failed or waiting for your approval; each sound and each notification is switched on its own in Settings → Notifications
- 2026-09-26 Commits the agent makes carry a Co-authored-by: Kibo trailer
- 2026-09-26 A / command shows in the chat as you typed it, and names the chat that way, while the agent gets the full prompt behind it
- 2026-09-26 Run /init to have the agent write AGENTS.md and CLAUDE.md for the open repository, or propose edits to the ones it already has
- 2026-09-26 Write your own / commands as Markdown files in .kibo/commands of the project or of ~/.kibo; $ARGUMENTS takes what is typed after the name
- 2026-09-26 Press ⌘L to jump to the message box; a new chat, from ⌘N or the button, puts the cursor there too
- 2026-09-26 A compacted chat keeps the name of its first message instead of being renamed after its summary; chats already renamed get their name back
- 2026-09-26 See history compaction in the chat as a card: under way while the summary is made, then how many messages it folded or that it gave up
- 2026-09-26 Type / in the message box for commands: /compact folds older history, /fork continues in a copy of the chat
- 2026-09-26 See in the context window panel what project rules, skills and MCP servers take, each on its own row
- 2026-09-26 See what share of the last request came from the provider's prompt cache in the context window panel
