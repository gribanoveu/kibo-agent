"""Orders behind a bearer token; an order that does not exist answers 200 with an
empty order, where the spec says 404."""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from lib import serve

TOKEN = "s3cr3t-st4tus-t0k"
ORDERS = {"42": {"id": 42, "status": "paid", "items": [{"sku": "A1", "qty": 2}]}}


def answer(method, path, query, headers, body):
    if headers.get("authorization") != f"Bearer {TOKEN}":
        return 401, {"error": "a bearer token is required"}, False
    if method not in ("GET", "HEAD"):
        return 405, {"error": "read only"}, False
    path = path.rstrip("/")
    if path == "/orders":
        return 200, list(ORDERS.values()), False
    if path.startswith("/orders/") and path.count("/") == 2:
        id = path.split("/")[2]
        if id in ORDERS:
            return 200, ORDERS[id], False
        if not id.isdigit():
            return 404, {"error": "no such order"}, False
        return 200, {"id": int(id), "status": "open", "items": []}, True
    return 404, {"error": "no such route"}, False


serve(answer)
