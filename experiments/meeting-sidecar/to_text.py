"""The sidecar's result as a transcript a person corrects into the reference.

    python3 to_text.py recordings/x.jsonl [offset-seconds] > recordings/x-reference.txt

The offset is where a cut-out piece starts in the whole recording, so its
times read the same as the full transcript's.

One line per utterance:

    [00:01:23.40 - 00:01:30.10] Спикер 3: текст

To make it the reference: fix the text, rename speakers to real names (the same
name everywhere), move the times where a speaker change is off, split a line
where two people are in it. Lines deleted are speech that is not there.
The format stays as is, so the corrected file can be scored against later runs.
"""

import json
import sys


def stamp(seconds: float) -> str:
    h, rest = divmod(seconds, 3600)
    m, s = divmod(rest, 60)
    return f"{int(h):02}:{int(m):02}:{s:05.2f}"


result = json.loads(open(sys.argv[1], encoding="utf-8").read().splitlines()[-1])
if result["type"] != "result":
    sys.exit(f"no result in {sys.argv[1]}: {result}")
offset = float(sys.argv[2]) if len(sys.argv) > 2 else 0.0
for u in result["utterances"]:
    print(f"[{stamp(offset + u['start'])} - {stamp(offset + u['end'])}] Спикер {u['speaker'] + 1}: {u['text']}")
