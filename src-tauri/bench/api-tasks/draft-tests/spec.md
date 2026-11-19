# Orders API

Requests carry `Authorization: Bearer <token>`; without it the answer is 401.

## GET /orders/{id}

- 200 — the order: `{"id": number, "status": "paid" | "open", "sum": number}`.
- 404 — `{"error": string}` when there is no order with that id.
