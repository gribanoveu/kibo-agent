"""DER and WER of sidecar results against a reference corrected from `to_text.py`.

    python3 score.py recordings/x-reference.txt recordings/run1.jsonl [run2.jsonl ...]

The reference is `to_text.py`'s format; lines that are not `[start - end] Name: text`
(a legend of names at the top) are skipped, and a speaker written as `?` is
speech whose owner is not known — counted for miss and false alarm, never for
confusion.

DER: 10 ms frames, no collar, overlap counted once per frame by the reference's
first speaker there. Hypothesis speakers map to reference ones one-to-one by
most shared time. WER: words of the whole recording in time order, lower case,
`ё` as `е`, punctuation and hyphens dropped.

Both are relative to a reference made by correcting one hypothesis: its
boundaries came from that run, so that run is favoured on DER.
"""

import json
import re
import sys

LINE = re.compile(r"^\[(\d+):(\d+):(\d+(?:\.\d+)?) - (\d+):(\d+):(\d+(?:\.\d+)?)\]\s*([^:]*):\s*(.*)$")


def seconds(h, m, s):
    return int(h) * 3600 + int(m) * 60 + float(s)


def reference(path):
    out = []
    for raw in open(path, encoding="utf-8"):
        m = LINE.match(raw.strip())
        if m:
            g = m.groups()
            out.append((g[6].strip() or "?", seconds(*g[0:3]), seconds(*g[3:6]), g[7]))
    return out


def hypothesis(path):
    result = json.loads(open(path, encoding="utf-8").read().splitlines()[-1])
    return [(str(u["speaker"]), u["start"], u["end"], u["text"]) for u in result["utterances"]]


def frames(lines, n):
    f = [None] * n
    for who, start, end, _ in lines:
        for i in range(int(start * 100), min(int(end * 100), n)):
            if f[i] is None:
                f[i] = who
    return f


def der(ref, hyp):
    n = int(max(e for _, _, e, _ in ref + hyp) * 100) + 1
    r, h = frames(ref, n), frames(hyp, n)
    shared = {}
    for a, b in zip(r, h):
        if a not in (None, "?") and b is not None:
            shared[(a, b)] = shared.get((a, b), 0) + 1
    # ponytail: greedy one-to-one map, not Hungarian; exact once two clusters tie for a person.
    mapping, used = {}, set()
    for (a, b), _ in sorted(shared.items(), key=lambda kv: -kv[1]):
        if b not in mapping and a not in used:
            mapping[b] = a
            used.add(a)
    speech = sum(1 for a in r if a is not None)
    miss = sum(1 for a, b in zip(r, h) if a is not None and b is None)
    fa = sum(1 for a, b in zip(r, h) if a is None and b is not None)
    conf = sum(1 for a, b in zip(r, h) if a not in (None, "?") and b is not None and mapping.get(b) != a)
    return 100 * (miss + fa + conf) / speech, 100 * miss / speech, 100 * fa / speech, 100 * conf / speech


def words(lines):
    text = " ".join(t for _, _, _, t in sorted(lines, key=lambda x: x[1]))
    text = text.lower().replace("ё", "е")
    return re.findall(r"[\w]+", text.replace("-", " "))


def wer(ref, hyp):
    a, b = words(ref), words(hyp)
    prev = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        cur = [i]
        for j, y in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (x != y)))
        prev = cur
    return 100 * prev[-1] / len(a), len(a)


def check():
    ref = [("A", 0.0, 1.0, "Привет, мир"), ("B", 1.0, 2.0, "ёлка")]
    assert der(ref, [("0", 0.0, 1.0, ""), ("1", 1.0, 2.0, "")])[0] == 0
    assert round(der(ref, [("0", 0.0, 2.0, "")])[3]) == 50  # B's second is confusion
    assert round(der(ref, [("0", 0.0, 1.0, "")])[1]) == 50  # B's second is missed
    assert wer(ref, [("0", 0.0, 2.0, "привет мир елка")])[0] == 0
    assert round(wer(ref, [("0", 0.0, 2.0, "привет")])[0]) == 67


if __name__ == "__main__":
    check()
    ref = reference(sys.argv[1])
    speakers = sorted({w for w, *_ in ref})
    print(f"reference: {len(ref)} lines, speakers {speakers}, {sum(e - s for _, s, e, _ in ref) / 60:.1f} min of speech")
    for path in sys.argv[2:]:
        hyp = hypothesis(path)
        d, miss, fa, conf = der(ref, hyp)
        w, n = wer(ref, hyp)
        print(f"{path.rsplit('/', 1)[-1]:<34} DER {d:5.1f}% (miss {miss:4.1f}, FA {fa:4.1f}, conf {conf:4.1f})  WER {w:5.1f}% of {n} words")
