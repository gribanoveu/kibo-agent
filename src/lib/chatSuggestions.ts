import type { ChatRoleId } from "./chat";

/**
 * What Chat mode's empty chat offers to ask — the questions of a Spring
 * developer at a bank, of system and API design, of architecture and of a
 * systems analyst. Short enough for one
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
  // Architecture.
  "Modular monolith or microservices for a new product?",
  "When does CQRS pay off, and when is it overkill?",
  "Event sourcing for account balances: worth it?",
  "Hexagonal architecture in Spring: how to lay out packages?",
  "How do I split a monolith into services safely?",
  "Write an ADR for choosing Kafka over RabbitMQ",
  "Multi-tenant SaaS: one database or one per tenant?",
  "Strangler fig: how do I start replacing a legacy system?",
  "Where does an API gateway end and a BFF begin?",
  "Draw a C4 container diagram for a payment system",
  "Bounded contexts for a banking app: where to draw lines?",
  "Choreography or orchestration for a multi-step flow?",
  "What does 99.99% uptime really take?",
  "Database per service: how do I report across them?",
  "Shared library or duplicated code between services?",
  "Read replicas or a cache for a read-heavy service?",
  "How do I make a service stateless to scale out?",
  // More Spring.
  "@Async in Spring: thread pools and pitfalls",
  "Spring WebFlux or MVC with virtual threads?",
  "How do Spring Boot auto-configurations work?",
  "Structure settings with @ConfigurationProperties",
  "Add tracing with Micrometer and OpenTelemetry",
  "Spring Cache with Redis: TTL and eviction done right",
  // Systems analysis.
  "Write user stories with acceptance criteria for a transfer",
  "Which non-functional requirements does a payment API need?",
  "Draw a sequence diagram for a card payment",
  "Model a loan application process in BPMN",
  "Use case or user story: when to use which?",
  "Design an ER model for customers, accounts and cards",
  "What goes into an OpenAPI spec before coding starts?",
];

/**
 * The Kubernetes role's: questions about the cluster the chat is pinned to.
 * Reads only — a chat starts on Read only, and a suggestion should work there.
 */
export const KUBE_SUGGESTIONS = [
  "Why is a pod in this namespace restarting?",
  "Which pods are not Ready, and why?",
  "Find the pod that was OOMKilled and say why",
  "Why is a pod stuck in Pending?",
  "What is behind a CrashLoopBackOff here?",
  "Why can't a pod pull its image?",
  "Is the last rollout healthy?",
  "What changed in this namespace in the last hour?",
  "Show the warning events in this namespace",
  "Which pods use the most memory right now?",
  "Are the requests and limits sensible here?",
  "Check the liveness and readiness probes",
  "Why does a service have no endpoints?",
  "Which CronJobs failed recently?",
  "Why is a Job not finishing?",
  "Is the autoscaler scaling as it should?",
  "Find the errors in the logs since the last deploy",
  "Explain what runs in this namespace",
];

/** Each role's set. */
export const ROLE_SUGGESTIONS: Record<ChatRoleId, readonly string[]> = {
  assistant: CHAT_SUGGESTIONS,
  kubernetes: KUBE_SUGGESTIONS,
};

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

/** `count` different suggestions from `set`, in random order. `random` is `Math.random` outside a test. */
export function pickSuggestions(set: readonly string[], count: number, random: () => number = Math.random): string[] {
  const pool = [...set];
  // The first `count` steps of a Fisher–Yates shuffle: each pick is uniform over what is left.
  for (let i = 0; i < Math.min(count, pool.length); i++) {
    const j = i + Math.floor(random() * (pool.length - i));
    [pool[i], pool[j]] = [pool[j], pool[i]];
  }
  return pool.slice(0, count);
}
