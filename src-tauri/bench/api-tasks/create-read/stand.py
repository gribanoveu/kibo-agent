"""Orders made and read back behind a bearer token; ids start at 41. No bug: the task is the chain."""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from lib import json_body, serve

TOKEN = "ch41n-t0k-41"
made = {}


def answer(method, path, query, headers, body):
    if headers.get("authorization") != f"Bearer {TOKEN}":
        return 401, {"error": "a bearer token is required"}, False
    parts = path.strip("/").split("/")
    if parts == ["orders"] and method == "POST":
        sent = json_body(body)
        if not isinstance(sent, dict) or isinstance(sent.get("sum"), bool) or not isinstance(sent.get("sum"), (int, float)) or sent["sum"] <= 0:
            return 422, {"error": "sum must be a positive number"}, False
        order = {"id": 41 + len(made), "sum": sent["sum"], "status": "open"}
        made[str(order["id"])] = order
        return 201, order, False
    if len(parts) == 2 and parts[0] == "orders" and method in ("GET", "HEAD"):
        order = made.get(parts[1])
        return (200, order, False) if order else (404, {"error": "no such order"}, False)
    return 404, {"error": "no such route"}, False


serve(answer)
