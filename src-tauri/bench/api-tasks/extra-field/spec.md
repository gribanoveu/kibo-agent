# Users API

Every request carries `Authorization: Bearer <token>`; without it the answer is 401.

## GET /users/{id}

- 200 — the user, exactly these fields: `{"id": number, "name": string, "email": string}`.
- 404 — `{"error": string}` when there is no user with that id.
