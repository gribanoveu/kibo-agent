"""Orders behind a token got by POST /login; the list's `status` filter is ignored —
every order comes back, where the spec says only those with that status."""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from lib import json_body, serve

USER, PASSWORD, TOKEN = "bench", "p4ssw0rd-b3nch", "l0gin-t0k-ab12"
ORDERS = [{"id": 1, "status": "paid"}, {"id": 2, "status": "open"}, {"id": 3, "status": "paid"}]


def answer(method, path, query, headers, body):
    path = path.rstrip("/")
    if path == "/login":
        sent = json_body(body)
        if method == "POST" and isinstance(sent, dict) and sent.get("user") == USER and sent.get("password") == PASSWORD:
            return 200, {"token": TOKEN}, False
        return 401, {"error": "wrong user or password"}, False
    if headers.get("authorization") != f"Bearer {TOKEN}":
        return 401, {"error": "a bearer token is required"}, False
    if path != "/orders" or method not in ("GET", "HEAD"):
        return 404, {"error": "no such route"}, False
    status = query.get("status", [None])[0]
    return 200, ORDERS, status is not None and any(o["status"] != status for o in ORDERS)


serve(answer)
