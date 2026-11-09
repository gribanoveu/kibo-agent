"""Users behind a bearer token; a user comes back with `passwordHash` besides the
three fields the spec lists. An unknown id answers 404, as said."""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from lib import serve

TOKEN = "s3cr3t-us3r-t0k"
USERS = {"7": {"id": 7, "name": "Ada", "email": "ada@example.test"}, "8": {"id": 8, "name": "Bo", "email": "bo@example.test"}}


def answer(method, path, query, headers, body):
    if headers.get("authorization") != f"Bearer {TOKEN}":
        return 401, {"error": "a bearer token is required"}, False
    if method not in ("GET", "HEAD"):
        return 405, {"error": "read only"}, False
    parts = path.rstrip("/").split("/")
    if len(parts) == 3 and parts[1] == "users":
        user = USERS.get(parts[2])
        if user is None:
            return 404, {"error": "no such user"}, False
        return 200, dict(user, passwordHash="$2b$12$Vq3eYbNrKjzEXAMPLEonly"), True
    return 404, {"error": "no such route"}, False


serve(answer)
