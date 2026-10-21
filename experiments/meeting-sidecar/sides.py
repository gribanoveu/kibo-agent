"""How well the words were put on their side, against where the sound really was:
sides.py ref.txt "Спикер 1" run.jsonl... — the mic is that speaker's reference spans ±0.6 s, as make_call.py makes it with pad 0.6."""
import json, re, sys
L = re.compile(r"^\[(\d+):(\d+):(\d+(?:\.\d+)?) - (\d+):(\d+):(\d+(?:\.\d+)?)\]\s*([^:]*):")
sec = lambda h, m, s: int(h) * 3600 + int(m) * 60 + float(s)
spans = []
for line in open(sys.argv[1], encoding="utf-8"):
    m = L.match(line.strip())
    if m and m.group(7).strip() == sys.argv[2]:
        g = m.groups(); spans.append((sec(*g[0:3]) - 0.6, sec(*g[3:6]) + 0.6))
def mic_share(a, b):
    return sum(max(0, min(b, e) - max(a, s)) for s, e in spans) / max(b - a, 1e-6)
for run in sys.argv[3:]:
    us = json.loads(open(run).read().splitlines()[-1])["utterances"]
    words = lambda u: len(u["text"].split())
    by = {}
    for u in us:
        by.setdefault(u["speaker"], [0, 0])
        by[u["speaker"]][0] += words(u) * mic_share(u["start"], u["end"]); by[u["speaker"]][1] += words(u)
    me = max(by, key=lambda k: by[k][0])
    mic_words = sum(v[0] for v in by.values())
    print(f"{run.split('/')[-1]:28} you = H{me}: {by[me][1]} words, {by[me][0]:.0f} of them from the mic; "
          f"mic words elsewhere {mic_words - by[me][0]:.0f} of {mic_words:.0f}; call words as yours {by[me][1] - by[me][0]:.0f}")
