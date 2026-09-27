# Changelog

## Unreleased

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
