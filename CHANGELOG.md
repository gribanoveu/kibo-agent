# Changelog

## Unreleased

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
