/**
 * What Chat mode's empty chat offers to ask. Short enough for one line each:
 * two rows is what keeps Kibo where the agent's empty chat has it.
 */
export const CHAT_SUGGESTIONS = [
  "Explain the difference between a process and a thread",
  "Write a regex that matches an ISO 8601 date",
  "Make this commit message clearer for a reviewer",
  "What does CAP theorem mean in practice?",
  "Compare REST and gRPC for internal services",
  "Explain Rust lifetimes with a small example",
  "How does a B-tree index speed up a query?",
  "Write a bash one-liner to find large files",
  "What is the difference between TCP and UDP?",
  "Explain how HTTPS keeps a connection private",
  "Suggest names for a CLI that syncs dotfiles",
  "Turn these notes into a short status update",
  "How do I undo the last git commit safely?",
  "Explain eventual consistency to a new teammate",
  "What makes a good code review comment?",
  "Write a SQL query for the top 5 customers by spend",
  "Explain async/await in JavaScript simply",
  "When should I use a queue instead of a direct call?",
  "Review this error message for clarity",
  "Summarize the SOLID principles in one line each",
];

/** `count` different suggestions, in random order. `random` is `Math.random` outside a test. */
export function pickSuggestions(count: number, random: () => number = Math.random): string[] {
  const pool = [...CHAT_SUGGESTIONS];
  // The first `count` steps of a Fisher–Yates shuffle: each pick is uniform over what is left.
  for (let i = 0; i < Math.min(count, pool.length); i++) {
    const j = i + Math.floor(random() * (pool.length - i));
    [pool[i], pool[j]] = [pool[j], pool[i]];
  }
  return pool.slice(0, count);
}
