#!/usr/bin/env python3
"""Rebuilds src/lib/providerCatalog.json from models.dev (MIT, the list OpenCode uses).

    python3 scripts/update-provider-catalog.py            # fetch https://models.dev/api.json
    python3 scripts/update-provider-catalog.py api.json   # or read a saved copy

Keeps what the provider form can fill in: a name, a protocol and a base URL.
A provider models.dev lists without a URL (its SDK knows it) is kept only when
it is in KNOWN below; one that speaks neither protocol is left out.
"""

import json
import sys
import urllib.request
from pathlib import Path

SOURCE = "https://models.dev/api.json"
OUT = Path(__file__).resolve().parent.parent / "src" / "lib" / "providerCatalog.json"

# models.dev leaves `api` empty where the provider's own SDK carries the URL.
# These serve the OpenAI-compatible (or, for Anthropic, its own) protocol at:
KNOWN = {
    "openai": "https://api.openai.com/v1",
    "anthropic": "https://api.anthropic.com/v1",
    "google": "https://generativelanguage.googleapis.com/v1beta/openai",
    "groq": "https://api.groq.com/openai/v1",
    "xai": "https://api.x.ai/v1",
    "mistral": "https://api.mistral.ai/v1",
    "togetherai": "https://api.together.xyz/v1",
    "deepinfra": "https://api.deepinfra.com/v1/openai",
    "cerebras": "https://api.cerebras.ai/v1",
}


def kind(npm: str) -> str:
    return "anthropic" if npm.endswith("/anthropic") else "openAiCompatible"


def main() -> None:
    if len(sys.argv) > 1:
        data = json.loads(Path(sys.argv[1]).read_text())
    else:
        # Python's own User-Agent is refused (403); curl's is not.
        request = urllib.request.Request(SOURCE, headers={"User-Agent": "curl/8"})
        with urllib.request.urlopen(request, timeout=60) as response:
            data = json.load(response)

    catalog = []
    for pid, provider in data.items():
        url = provider.get("api") or KNOWN.get(pid)
        if not url or not url.startswith("https://"):
            continue
        catalog.append(
            {
                "id": pid,
                "name": provider.get("name") or pid,
                "kind": kind(provider.get("npm") or ""),
                "baseUrl": url.rstrip("/"),
            }
        )
    catalog.sort(key=lambda p: p["name"].lower())
    OUT.write_text(json.dumps(catalog, indent=1, ensure_ascii=False) + "\n")
    print(f"{len(catalog)} providers -> {OUT}")


if __name__ == "__main__":
    main()
