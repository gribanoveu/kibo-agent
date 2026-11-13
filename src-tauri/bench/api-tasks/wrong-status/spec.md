# Orders API

Every request carries `Authorization: Bearer <token>`; without it the answer is 401.

## GET /orders

200 — a JSON array of orders.

## GET /orders/{id}

- 200 — the order: `{"id": number, "status": "paid" | "open", "items": [{"sku": string, "qty": number}]}`.
- 404 — `{"error": string}` when there is no order with that id.
