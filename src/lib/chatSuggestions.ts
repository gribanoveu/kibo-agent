/** The empty chat's heading: one of these, or of the time of day's, as Claude greets. */
export const GREETINGS = [
  "How can I help?",
  "How can I help you today?",
  "What's on your mind?",
  "What are we working on?",
  "Where should we start?",
  "What can I do for you?",
];

/** By the hour the chat opens: before 5 is still the night. */
export const GREETINGS_BY_TIME = {
  morning: ["Good morning", "Morning! What's first?"],
  afternoon: ["Good afternoon", "Afternoon! What's next?"],
  evening: ["Good evening", "Evening! What are we looking at?"],
  night: ["Up late?", "Burning the midnight oil?"],
};

export function greetingsAt(hour: number): readonly string[] {
  const time = hour < 5 ? "night" : hour < 12 ? "morning" : hour < 18 ? "afternoon" : hour < 23 ? "evening" : "night";
  return [...GREETINGS, ...GREETINGS_BY_TIME[time]];
}

/** `count` different ones from `set`, in random order. `random` is `Math.random` outside a test. */
export function pickSuggestions(set: readonly string[], count: number, random: () => number = Math.random): string[] {
  const pool = [...set];
  // The first `count` steps of a Fisher–Yates shuffle: each pick is uniform over what is left.
  for (let i = 0; i < Math.min(count, pool.length); i++) {
    const j = i + Math.floor(random() * (pool.length - i));
    [pool[i], pool[j]] = [pool[j], pool[i]];
  }
  return pool.slice(0, count);
}
