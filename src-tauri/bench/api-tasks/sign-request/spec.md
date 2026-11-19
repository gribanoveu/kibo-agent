# Payments API

Requests carry `Authorization: Bearer <token>`.

## POST /payments

- Body `{"sum": number, "currency": string}`.
- Header `X-Signature`: the HMAC-SHA256 of the raw body, keyed with the environment's `signingKey`, as lowercase hex.
- 201 — the payment made; 401 — the signature is missing or wrong.
