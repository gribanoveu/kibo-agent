# Orders API

Requests carry `Authorization: Bearer <token>`; without it the answer is 401.

## POST /orders

- Body `{"sum": number}`, a positive number.
- 201 — the order made: `{"id": number, "sum": number, "status": "open"}`.

## GET /orders/{id}

- 200 — the order with that id, as it was made.
- 404 — `{"error": string}` when there is none.
