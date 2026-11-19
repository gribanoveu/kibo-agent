import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from lib import check_drafts, draft_of, judged_on_variants


def judge(drafts, reruns, notes):
    faults = []
    if draft_of(drafts, "order-create", "test") is None:
        faults.append("no post-response draft of order-create")
    read = draft_of(drafts, "order-get", "test")
    if read is None:
        return faults + ["no post-response draft of order-get"]
    # The id goes from the create into the read's run, as `orderId`: the read's variants stand on
    # the send of that order, and `elsewhere` gives another — an id written in, or none, fails.
    faults += judged_on_variants(reruns, "order-create", {"same": "pass", "notCreated": "fail"})
    faults += judged_on_variants(reruns, "order-get", {"same": "pass", "elsewhere": "pass", "another": "fail"})
    return faults


check_drafts(judge)
