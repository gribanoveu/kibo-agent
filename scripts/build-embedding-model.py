#!/usr/bin/env python3
"""Quantize minishlab/potion-multilingual-128M to int8 and write a Rust parity fixture.

Does not use third-party int8 reuploads. The revision is NOT pinned: HF_REVISION
is "main" and is only recorded in parity.json, never passed to the download, so a
rebuild takes whatever `main` is on that day. Upstream's docstring claimed
otherwise. Pin it to a commit hash before the
next rebuild, and pass it through.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

HF_REPO = "minishlab/potion-multilingual-128M"
HF_REVISION = "main"
OUT_REL = Path("src-tauri/models/potion-multilingual-128M-int8")

PARITY_PHRASES = [
    "документация REST API",
    "открыть расчётный счёт",
    "Hello world",
    "getUserById",
    "camelCaseIdentifier",
    "ошибка валидации платежа",
    "OpenAPI specification",
    "уведомления по операции",
]


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def cosine(a: list[float], b: list[float]) -> float:
    dot = sum(x * y for x, y in zip(a, b, strict=True))
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(x * x for x in b))
    return dot / max(na * nb, 1e-12)


def build(out_dir: Path) -> None:
    from model2vec import StaticModel

    model = StaticModel.from_pretrained(
        HF_REPO,
        quantize_to="int8",
        token=None,
        force_download=False,
    )
    out_dir.mkdir(parents=True, exist_ok=True)
    model.save_pretrained(str(out_dir))

    vectors = model.encode(PARITY_PHRASES)
    fixture = {
        "repo": HF_REPO,
        "revision": HF_REVISION,
        "phrases": PARITY_PHRASES,
        "vectors": [row.tolist() for row in vectors],
    }
    (out_dir / "parity.json").write_text(
        json.dumps(fixture, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"wrote {out_dir} ({len(vectors[0])} dims)")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--out",
        type=Path,
        default=repo_root() / OUT_REL,
        help="directory for model.safetensors / tokenizer.json / config.json",
    )
    args = parser.parse_args()
    build(args.out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
