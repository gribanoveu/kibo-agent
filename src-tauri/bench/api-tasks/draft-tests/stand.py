"""One order behind a bearer token; its `sum` comes as a string, where the spec says a number."""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from lib import serve

TOKEN = "dr4ft-t0k-55"


def answer(method, path, query, headers, body):
    if headers.get("authorization") != f"Bearer {TOKEN}":
        return 401, {"error": "a bearer token is required"}, False
    if method not in ("GET", "HEAD"):
        return 405, {"error": "GET only"}, False
    if path.rstrip("/") == "/orders/5":
        return 200, {"id": 5, "status": "paid", "sum": "120.00"}, True
    return 404, {"error": "no such order"}, False


serve(answer)
