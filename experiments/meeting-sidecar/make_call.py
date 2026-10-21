"""A recorded call from a mono meeting and its reference: one reference speaker
on the left (the microphone), everyone else on the right (the call), the left
hearing the right back `echo` as loud, 50 ms late, over a quiet room.

    python3 make_call.py in.wav reference.txt "Спикер 1" echo out.wav [pad]

The left is that speaker's reference spans widened by `pad` seconds (0.15): a
reference's times are a person's, and speech of theirs past them stays on the right.
"""
import array, random, re, sys, wave

src, ref, me, echo, out = sys.argv[1], sys.argv[2], sys.argv[3], float(sys.argv[4]), sys.argv[5]
pad = float(sys.argv[6]) if len(sys.argv) > 6 else 0.15
L = re.compile(r"^\[(\d+):(\d+):(\d+(?:\.\d+)?) - (\d+):(\d+):(\d+(?:\.\d+)?)\]\s*([^:]*):")
sec = lambda h, m, s: int(h) * 3600 + int(m) * 60 + float(s)
with wave.open(src) as w:
    assert w.getnchannels() == 1 and w.getframerate() == 16000 and w.getsampwidth() == 2
    x = array.array("h", w.readframes(w.getnframes()))
n, rate = len(x), 16000
mine = bytearray(n)
for line in open(ref, encoding="utf-8"):
    m = L.match(line.strip())
    if m and m.group(7).strip() == me:
        g = m.groups()
        a, b = max(0, int((sec(*g[0:3]) - pad) * rate)), min(n, int((sec(*g[3:6]) + pad) * rate))
        mine[a:b] = b"\x01" * (b - a)
call = [0 if mine[i] else x[i] for i in range(n)]
lag = int(0.05 * rate)
random.seed(1)
mic = []
for i in range(n):
    v = (0.7 * x[i] if mine[i] else 0.0) + echo * (call[i - lag] if i >= lag else 0) + random.gauss(0, 30)
    mic.append(max(-32767, min(32767, int(v))))
frames = array.array("h", (s for pair in zip(mic, call) for s in pair))
with wave.open(out, "wb") as w:
    w.setnchannels(2); w.setsampwidth(2); w.setframerate(rate); w.writeframes(frames.tobytes())
print(out, n / rate, "s,", sum(mine) / rate, "s mine")
