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
boundaries came from that run, so that run is favoured on DER — and speech that
run missed but a person wrote down has wrong times, so DER over-counts false
alarm for a run that finds it. WDER, over aligned words, needs no times.
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
    """Utterances as lines; one that lists its words' spans is speech only there,
    with pauses under 0.5 s closed — what diarization's `min_duration_off` does."""
    result = json.loads(open(path, encoding="utf-8").read().splitlines()[-1])
    out = []
    for u in result["utterances"]:
        spans = []
        for s, e in u.get("words") or [(u["start"], u["end"])]:
            if spans and s - spans[-1][1] < 0.5:
                spans[-1][1] = e
            else:
                spans.append([s, e])
        out += [(str(u["speaker"]), s, e, u["text"] if i == 0 else "") for i, (s, e) in enumerate(spans)]
    return out


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


def spoken(lines):
    """(word, speaker) of the whole recording in time order."""
    out = []
    for who, _, _, text in sorted(lines, key=lambda x: x[1]):
        text = text.lower().replace("ё", "е").replace("-", " ")
        out += [(w, who) for w in re.findall(r"\w+", text)]
    return out


def words(lines):
    return [w for w, _ in spoken(lines)]


def align(a, b):
    """Edit-distance alignment: the distance and the (i, j) pairs matched or substituted."""
    d = [[0] * (len(b) + 1) for _ in range(len(a) + 1)]
    for i in range(len(a) + 1):
        d[i][0] = i
    for j in range(len(b) + 1):
        d[0][j] = j
    for i in range(1, len(a) + 1):
        for j in range(1, len(b) + 1):
            d[i][j] = min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + (a[i - 1] != b[j - 1]))
    pairs, i, j = [], len(a), len(b)
    while i and j:
        if d[i][j] == d[i - 1][j - 1] + (a[i - 1] != b[j - 1]):
            pairs.append((i - 1, j - 1))
            i, j = i - 1, j - 1
        elif d[i][j] == d[i - 1][j] + 1:
            i -= 1
        else:
            j -= 1
    return d[-1][-1], pairs[::-1]


def wer(ref, hyp):
    a, b = words(ref), words(hyp)
    return 100 * align(a, b)[0] / len(a), len(a)


def wder(ref, hyp):
    """Word diarization error: of the words both sides have, the share given to
    the wrong speaker under the best one-to-one map. Needs no reference times —
    the measure for a reference whose text was fixed but not its boundaries."""
    a, b = spoken(ref), spoken(hyp)
    pairs = [(a[i][1], b[j][1]) for i, j in align([w for w, _ in a], [w for w, _ in b])[1] if a[i][1] != "?"]
    counts = {}
    for p in pairs:
        counts[p] = counts.get(p, 0) + 1
    mapping, used = {}, set()
    for (r, h), _ in sorted(counts.items(), key=lambda kv: -kv[1]):
        if h not in mapping and r not in used:
            mapping[h] = r
            used.add(r)
    return 100 * sum(1 for r, h in pairs if mapping.get(h) != r) / len(pairs)


def check():
    ref = [("A", 0.0, 1.0, "Привет, мир"), ("B", 1.0, 2.0, "ёлка")]
    assert der(ref, [("0", 0.0, 1.0, ""), ("1", 1.0, 2.0, "")])[0] == 0
    assert round(der(ref, [("0", 0.0, 2.0, "")])[3]) == 50  # B's second is confusion
    assert round(der(ref, [("0", 0.0, 1.0, "")])[1]) == 50  # B's second is missed
    assert wer(ref, [("0", 0.0, 2.0, "привет мир елка")])[0] == 0
    assert round(wer(ref, [("0", 0.0, 2.0, "привет")])[0]) == 67
    assert wder(ref, [("7", 0.0, 1.0, "привет мир"), ("3", 1.0, 2.0, "елка")]) == 0
    assert round(wder(ref, [("7", 0.0, 2.0, "привет мир елка")])) == 33  # "елка" is B's


if __name__ == "__main__":
    check()
    ref = reference(sys.argv[1])
    speakers = sorted({w for w, *_ in ref})
    print(f"reference: {len(ref)} lines, speakers {speakers}, {sum(e - s for _, s, e, _ in ref) / 60:.1f} min of speech")
    for path in sys.argv[2:]:
        hyp = hypothesis(path)
        d, miss, fa, conf = der(ref, hyp)
        w, n = wer(ref, hyp)
        print(f"{path.rsplit('/', 1)[-1]:<34} WER {w:5.1f}% of {n} words  WDER {wder(ref, hyp):5.1f}%  "
              f"DER {d:5.1f}% (miss {miss:4.1f}, FA {fa:4.1f}, conf {conf:4.1f})")
