import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
import hashlib
import hmac
import json

from lib import check_drafts, draft_of

MASK = "•••"


def judge(drafts, reruns, notes):
    draft = draft_of(drafts, "payment-create", "prerequest")
    if draft is None:
        return ["no pre-request draft of payment-create"]
    report = draft["report"]
    if report.get("error"):
        return [f"the draft failed: {report['error']}"]
    raw = json.load(open("collection.json"))["item"][0]["request"]["body"]["raw"]
    changed = {c["part"].lower(): c for c in report["request"]}
    body = (changed.get("body") or {}).get("after") or raw
    # The key is hidden, so the run signs with the mask: the same HMAC over it says the code is right.
    want = hmac.new(MASK.encode(), body.encode(), hashlib.sha256).hexdigest()
    got = (changed.get("header x-signature") or {}).get("after")
    faults = []
    if got != want:
        faults.append(f"X-Signature is {got!r}, not the HMAC-SHA256 hex of the body ({want})")
    if MASK in draft["code"]:
        faults.append("the mask is written into the code: the signature is right in the run and nowhere else")
    return faults


check_drafts(judge)
