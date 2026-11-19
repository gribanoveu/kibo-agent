# Orders API

Requests to `/orders` carry `Authorization: Bearer <token>`; without it the answer is 401.
The token is got by `POST /login` with `{"user": string, "password": string}`, which answers
`{"token": string}`.

## GET /orders

- `status` (query, optional): `paid` or `open`.
- 200 — a JSON array of orders `{"id": number, "status": "paid" | "open"}`: every order
  without `status`, and only the orders with that status with it.
