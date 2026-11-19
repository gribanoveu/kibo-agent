# Orders API

Every request carries `Authorization: Bearer <token>`; without it the answer is 401.

## POST /orders

The body is a JSON object:

| Field | Type | Required | Rule |
|---|---|---|---|
| `sum` | number | yes | greater than 0 |
| `currency` | string | no | `EUR` when left out |

- 201 — the order created: `{"id": number, "sum": number, "currency": string}`.
- 422 — `{"error": string}` when `sum` is missing, is not a number, or is not greater than 0,
  or when `currency` is not a string.

## GET /orders

200 — the orders created, as a JSON array.
