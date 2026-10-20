// The models of meeting notes (docs/36-meeting-notes-research.md), bundled
// with the app: GigaAM v3 with punctuation, as sherpa-onnx exported it, and
// campplus for telling voices apart. Pinned to a commit and checked by hash;
// a file already here with the right hash is not fetched again.
//
//   bun scripts/fetch-meeting-models.ts [out dir]   (default: src-tauri/models/meeting)
//
// The names here are the ones `infra::speech::MODEL_FILES` reads.

import { mkdirSync, renameSync, existsSync } from "node:fs";
import { join, resolve } from "node:path";

const out = resolve(process.argv[2] ?? "src-tauri/models/meeting");
mkdirSync(out, { recursive: true });

const gigaam =
  "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-transducer-punct-giga-am-v3-russian-2025-12-16/resolve/a6039be7cee829a9044a69ac0ebaf1c191217c97";
const voices = "https://huggingface.co/csukuangfj/speaker-embedding-models/resolve/0743f301363dec56491a490f6d6cbc9d67f9a3bf";
// The voice model's licence, Apache 2.0 — its ModelScope card
// (iic/speech_campplus_sv_zh_en_16k-common_advanced) and the 3D-Speaker repo;
// the Hugging Face copy above carries none. Apache asks for it beside the weights.
const threeDSpeaker = "https://raw.githubusercontent.com/modelscope/3D-Speaker/065629c313eaf1a01c65c640c46d77e61e9607b4";

const files: [name: string, url: string, sha256: string][] = [
  ["encoder.int8.onnx", `${gigaam}/encoder.int8.onnx`, "369f35a71bf288d3b8e0391fabd8dba5f2314088d440bca474056b7b4b6e66bf"],
  ["decoder.onnx", `${gigaam}/decoder.onnx`, "38fc7475443ea2a26f63211ca350f73ac50fff824ab7a3876ee2bd610c53bbc4"],
  ["joiner.onnx", `${gigaam}/joiner.onnx`, "602ff7017a93311aad34df1437c8d7f49911353c13d6eae7a6ee7b041339465c"],
  ["tokens.txt", `${gigaam}/tokens.txt`, "39abae20e692998290c574e606f11a9edef2902a1995463fcff63d1490cf22b7"],
  ["GigaAM-LICENSE", `${gigaam}/LICENSE`, "f00de6715714c7a63d08639cdbfaa40224eefc407302614bd19f1a8b98c875aa"],
  [
    "campplus.onnx",
    `${voices}/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx`,
    "aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2",
  ],
  ["campplus-LICENSE", `${threeDSpeaker}/LICENSE`, "c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4"],
];

async function sha256(path: string): Promise<string> {
  const hasher = new Bun.CryptoHasher("sha256");
  hasher.update(await Bun.file(path).arrayBuffer());
  return hasher.digest("hex");
}

for (const [name, url, hash] of files) {
  const path = join(out, name);
  if (existsSync(path) && (await sha256(path)) === hash) continue;
  console.log(`fetching ${name}`);
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: ${response.status}`);
  const partial = `${path}.partial`;
  await Bun.write(partial, response);
  const got = await sha256(partial);
  if (got !== hash) throw new Error(`${name}: sha256 ${got}, expected ${hash}`);
  renameSync(partial, path);
}
console.log(`meeting models in ${out}`);
