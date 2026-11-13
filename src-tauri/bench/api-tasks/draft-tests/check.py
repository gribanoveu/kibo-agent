import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from lib import check_drafts, draft_of, judged_on_variants


def judge(drafts, reruns, notes):
    if draft_of(drafts, "order-get", "test") is None:
        return ["no post-response draft of order-get"]
    # Right on the fixed stand; caught on the planted bug and on the other wrongs.
    return judged_on_variants(reruns, "order-get", {"fixed": "pass", "planted": "fail", "badStatus": "fail"})


check_drafts(judge)
