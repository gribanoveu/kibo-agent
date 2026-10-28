// ponytail: sample data for the HTTP page while it has no backend — the
// page's look before HC-3 (docs/45-http-client.md). Goes when the commands come.

import type { Collection, Environment, HistoryEntry, HttpRequest, HttpResponse } from "./httpClient";

const TOKEN_SCRIPT = `// Gets a token for the stand once it is out of date.
const expires = pm.environment.get("tokenExpires");
if (!expires || Date.now() > Number(expires)) {
  pm.sendRequest({
    url: pm.environment.get("authUrl") + "/oauth/token",
    method: "POST",
    header: { "Content-Type": "application/x-www-form-urlencoded" },
    body: { mode: "urlencoded", urlencoded: [
      { key: "grant_type", value: "client_credentials" },
      { key: "client_id", value: pm.environment.get("clientId") },
      { key: "client_secret", value: pm.environment.get("clientSecret") },
    ] },
  }, (err, res) => {
    pm.environment.set("token", res.json().access_token);
    pm.environment.set("tokenExpires", Date.now() + res.json().expires_in * 1000);
  });
}`;

const CREATED_TEST = `pm.test("Created", () => pm.response.to.have.status(201));
pm.test("Has an id", () => pm.expect(pm.response.json().id).to.be.a("string"));
pm.collectionVariables.set("orderId", pm.response.json().id);`;

export const DEMO_COLLECTIONS: Collection[] = [
  {
    id: "orders",
    name: "Orders API",
    auth: { type: "bearer", token: "{{token}}" },
    variables: [{ key: "orderId", value: "ord_7Hq2" }],
    scripts: { prerequest: TOKEN_SCRIPT },
    items: [
      {
        kind: "folder",
        id: "orders-folder",
        name: "Orders",
        auth: { type: "inherit" },
        items: [
          {
            kind: "request",
            id: "list-orders",
            name: "List orders",
            method: "GET",
            url: "{{baseUrl}}/v1/orders?status=paid&limit=20",
            headers: [{ key: "Accept", value: "application/json" }],
            auth: { type: "inherit" },
            body: { mode: "none" },
            scripts: {},
          },
          {
            kind: "request",
            id: "get-order",
            name: "Get order",
            method: "GET",
            url: "{{baseUrl}}/v1/orders/{{orderId}}",
            headers: [{ key: "Accept", value: "application/json" }],
            auth: { type: "inherit" },
            body: { mode: "none" },
            scripts: {},
          },
          {
            kind: "request",
            id: "create-order",
            name: "Create order",
            method: "POST",
            url: "{{baseUrl}}/v1/orders",
            headers: [
              { key: "Content-Type", value: "application/json" },
              { key: "X-Request-Id", value: "{{$guid}}" },
              { key: "X-Debug", value: "1", disabled: true },
            ],
            auth: { type: "inherit" },
            body: { mode: "raw", language: "json", raw: '{\n  "customerId": "cus_19",\n  "amount": 1250.00,\n  "currency": "RUB",\n  "items": [{ "sku": "A-100", "qty": 2 }]\n}' },
            scripts: { test: CREATED_TEST },
          },
          {
            kind: "request",
            id: "cancel-order",
            name: "Cancel order",
            method: "DELETE",
            url: "{{baseUrl}}/v1/orders/{{orderId}}",
            headers: [],
            auth: { type: "inherit" },
            body: { mode: "none" },
            scripts: {},
          },
        ],
      },
      {
        kind: "request",
        id: "health",
        name: "Health",
        method: "GET",
        url: "{{baseUrl}}/actuator/health",
        headers: [],
        auth: { type: "noauth" },
        body: { mode: "none" },
        scripts: {},
      },
    ],
  },
  {
    id: "payments",
    name: "Payments",
    auth: { type: "other", name: "oauth2" },
    variables: [],
    scripts: {},
    items: [
      {
        kind: "request",
        id: "upload-receipt",
        name: "Upload receipt",
        method: "POST",
        url: "{{baseUrl}}/v2/receipts",
        headers: [],
        auth: { type: "inherit" },
        body: { mode: "other", name: "formdata" },
        scripts: {},
      },
      {
        kind: "request",
        id: "refund",
        name: "Refund",
        method: "PATCH",
        url: "{{baseUrl}}/v2/payments/{{paymentId}}",
        headers: [{ key: "Content-Type", value: "application/json" }],
        auth: { type: "inherit" },
        body: { mode: "raw", language: "json", raw: '{ "status": "refunded" }' },
        scripts: {},
      },
    ],
  },
];

export const DEMO_ENVIRONMENTS: Environment[] = [
  {
    id: "local",
    name: "local",
    agent: "allow",
    trustedCert: false,
    variables: [
      { key: "baseUrl", value: "http://localhost:8080", secret: false, enabled: true },
      { key: "token", value: "", secret: true, enabled: true },
    ],
  },
  {
    id: "test",
    name: "test",
    agent: "ask",
    trustedCert: true,
    variables: [
      { key: "baseUrl", value: "https://orders.test.corp.example", secret: false, enabled: true },
      { key: "authUrl", value: "https://sso.test.corp.example", secret: false, enabled: true },
      { key: "clientId", value: "orders-qa", secret: false, enabled: true },
      { key: "clientSecret", value: "", secret: true, enabled: true },
      { key: "token", value: "", secret: true, enabled: true },
      { key: "tokenExpires", value: "1793212800000", secret: false, enabled: true },
    ],
  },
  {
    id: "prod",
    name: "prod",
    agent: "off",
    trustedCert: false,
    variables: [{ key: "baseUrl", value: "https://orders.corp.example", secret: false, enabled: true }],
  },
];

const now = Date.now();
export const DEMO_HISTORY: HistoryEntry[] = [
  { id: "h1", at: now - 2 * 60_000, method: "POST", url: "{{baseUrl}}/v1/orders", status: 422, ms: 184, environment: "local", agent: { chat: "Fix order validation" } },
  { id: "h2", at: now - 3 * 60_000, method: "GET", url: "{{baseUrl}}/v1/orders?status=paid", status: 200, ms: 61, environment: "local", agent: { chat: "Fix order validation" } },
  { id: "h3", at: now - 26 * 60_000, method: "GET", url: "{{baseUrl}}/v1/orders/{{orderId}}", status: 200, ms: 92, environment: "test" },
  { id: "h4", at: now - 3 * 3600_000, method: "DELETE", url: "{{baseUrl}}/v1/orders/{{orderId}}", status: 204, ms: 77, environment: "test" },
  { id: "h5", at: now - 26 * 3600_000, method: "GET", url: "{{baseUrl}}/actuator/health", status: null, ms: 30_000, environment: "test" },
];

const json = (value: unknown) => JSON.stringify(value);

/** What the sample stand answers. */
export function demoResponse(request: HttpRequest, url: string): HttpResponse {
  const base = { ms: 40 + Math.round(Math.random() * 160), contentType: "application/json", tests: [] as HttpResponse["tests"] };
  const headers = (extra: [string, string][] = []) => [
    { key: "content-type", value: "application/json" },
    { key: "x-request-id", value: "3f1c9a20-5d1e-4c7a-9b0e-8c4f2d6e1a77" },
    { key: "date", value: new Date().toUTCString() },
    ...extra.map(([key, value]) => ({ key, value })),
  ];
  const done = (status: number, statusText: string, body: string, extra: Partial<HttpResponse> = {}): HttpResponse => ({
    ...base,
    status,
    statusText,
    body,
    bytes: new TextEncoder().encode(body).length,
    headers: headers(),
    ...extra,
  });
  if (request.method === "DELETE") return { ...done(204, "No Content", ""), contentType: "" };
  if (request.method === "POST" && /orders$/.test(url)) {
    const body = json({ id: "ord_8Kp4", status: "new", amount: 1250, currency: "RUB", createdAt: new Date().toISOString() });
    return done(201, "Created", body, {
      headers: headers([["location", "/v1/orders/ord_8Kp4"]]),
      tests: [
        { name: "Created", passed: true },
        { name: "Has an id", passed: true },
      ],
    });
  }
  if (/health$/.test(url)) return done(200, "OK", json({ status: "UP", components: { db: { status: "UP" }, kafka: { status: "UP" } } }));
  if (/orders\?/.test(url))
    return done(
      200,
      "OK",
      json({
        items: [
          { id: "ord_7Hq2", status: "paid", amount: 990.5, currency: "RUB", customer: { id: "cus_19", name: "ООО Ромашка" } },
          { id: "ord_7Hq9", status: "paid", amount: 15000, currency: "RUB", customer: { id: "cus_44", name: "ИП Иванов" } },
        ],
        total: 2,
        next: null,
      }),
    );
  return done(200, "OK", json({ id: "ord_7Hq2", status: "paid", amount: 990.5, currency: "RUB", lines: [{ sku: "A-100", qty: 1, price: 990.5 }] }));
}
