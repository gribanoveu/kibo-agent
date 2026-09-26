# Changelog

## Unreleased

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
