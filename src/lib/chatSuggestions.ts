/**
 * What Chat mode's empty chat offers to ask — the questions of a Spring
 * developer at a bank, and of system and API design. Short enough for one
 * line each: two rows is what keeps Kibo where the agent's empty chat has it.
 */
export const CHAT_SUGGESTIONS = [
  "Why is my @Transactional method not rolling back?",
  "Explain @Transactional propagation with examples",
  "Design an idempotent payment API in Spring Boot",
  "Optimistic vs pessimistic locking in JPA",
  "How do I fix the N+1 problem in Hibernate?",
  "BigDecimal for money: which pitfalls to avoid?",
  "Outbox pattern with Spring and Kafka, step by step",
  "Set up a Resilience4j circuit breaker in Spring",
  "Secure a REST API with Spring Security and JWT",
  "Mask card numbers and PII in Spring Boot logs",
  "Test a JPA repository with Testcontainers",
  "Liquibase or Flyway for a banking schema?",
  "Audit entity changes with Hibernate Envers",
  "Tune HikariCP for a high-load banking service",
  "Kafka consumer retries and a dead letter topic",
  "Saga or 2PC for transfers between services?",
  "Validate an IBAN with Bean Validation",
  "Run a @Scheduled job once across instances",
  "Migrate a Spring Boot 2 app to Spring Boot 3",
  "Virtual threads in Spring Boot: when do they help?",
  // System and API design.
  "Design a rate limiter for a public banking API",
  "How should I version a REST API without breaking clients?",
  "Cursor or offset pagination for transaction history?",
  "Design error responses with RFC 7807 problem details",
  "Sync REST or async events between two services?",
  "Design a ledger that never loses a cent",
  "How do I shard a table of accounts?",
  "Where should a cache sit in a card payment flow?",
  "Design webhooks that clients can trust and retry",
  "Idempotency keys: how long to keep them, and where?",
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
