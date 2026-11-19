"""Creating orders behind a bearer token; a body without `sum` is taken (201),
where the spec says 422. A `sum` that is not a positive number is refused, as said."""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from lib import json_body, serve

TOKEN = "s3cr3t-cr34te-t0k"
made = []


def answer(method, path, query, headers, body):
    if headers.get("authorization") != f"Bearer {TOKEN}":
        return 401, {"error": "a bearer token is required"}, False
    if path.rstrip("/") != "/orders":
        return 404, {"error": "no such route"}, False
    if method in ("GET", "HEAD"):
        return 200, made, False
    if method != "POST":
        return 405, {"error": "GET or POST"}, False
    order = json_body(body)
    if not isinstance(order, dict):
        return 400, {"error": "the body is not a JSON object"}, False
    if "sum" in order:
        sum = order["sum"]
        if isinstance(sum, bool) or not isinstance(sum, (int, float)) or sum <= 0:
            return 422, {"error": "sum must be a positive number"}, False
    if not isinstance(order.get("currency", "EUR"), str):
        return 422, {"error": "currency must be a string"}, False
    created = {"id": len(made) + 1, "sum": order.get("sum"), "currency": order.get("currency", "EUR")}
    made.append(created)
    return 201, created, "sum" not in order


serve(answer)
