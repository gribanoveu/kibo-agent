"""Payments signed with an HMAC of the body. The task is the signing script: the stand is there to be what the script is for."""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
import hashlib
import hmac

from lib import serve

TOKEN, KEY = "p4y-t0k-77", "s1gn-k3y-0f-th3-b3nch"


def answer(method, path, query, headers, body):
    if headers.get("authorization") != f"Bearer {TOKEN}":
        return 401, {"error": "a bearer token is required"}, False
    if path.rstrip("/") != "/payments" or method != "POST":
        return 404, {"error": "no such route"}, False
    if headers.get("x-signature") != hmac.new(KEY.encode(), body, hashlib.sha256).hexdigest():
        return 401, {"error": "the signature is missing or wrong"}, False
    return 201, {"id": 1}, False


serve(answer)
