// The models of meeting notes (docs/36-meeting-notes-research.md), bundled
// with the app: GigaAM v4 CTC with punctuation (ai-babai's ONNX export),
// campplus for telling voices apart, pyannote's segmentation for saying when
// each of them speaks, and DPDFNet-2 for taking the noise out of a recorded
// call's microphone, and Silero VAD for dictation's phrases. Pinned to a commit and checked by hash;
// a file already here with the right hash is not fetched again.
//
//   bun scripts/fetch-meeting-models.ts [out dir]   (default: src-tauri/models/meeting)
//
// The names here are the ones `infra::speech::MODEL_FILES`, `DICTATION_FILES`
// and `DIARIZATION_MODEL` read.

import { mkdirSync, renameSync, existsSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";

const out = resolve(process.argv[2] ?? "src-tauri/models/meeting");
mkdirSync(out, { recursive: true });

const gigaam =
  "https://huggingface.co/ai-babai/gigaam-v4-onnx/resolve/d39e2a200f596315acef7003d246b65a73e9a327/ctc-e2e";
const voices = "https://huggingface.co/csukuangfj/speaker-embedding-models/resolve/0743f301363dec56491a490f6d6cbc9d67f9a3bf";
// The voice model's licence, Apache 2.0 — its ModelScope card
// (iic/speech_campplus_sv_zh_en_16k-common_advanced) and the 3D-Speaker repo;
// the Hugging Face copy above carries none. Apache asks for it beside the weights.
const threeDSpeaker = "https://raw.githubusercontent.com/modelscope/3D-Speaker/065629c313eaf1a01c65c640c46d77e61e9607b4";
// sherpa-onnx's export of DPDFNet-2 (16 kHz). A release asset, not a commit:
// the hash is what holds it. Apache 2.0, as Ceva's repository says.
const denoiser = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speech-enhancement-models";
const dpdfnet = "https://raw.githubusercontent.com/ceva-ip/DPDFNet/9bd9844a227bb6aa57e55588d8d0e961fcff1c46";

// sherpa-onnx's copy of Silero VAD. A release asset, held by its hash; MIT, as Silero's repository says.
const sileroVad = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";
const silero = "https://raw.githubusercontent.com/snakers4/silero-vad/066409f3d9db45a30d21dfcbf9147826ffdd7052";
// pyannote's segmentation ships as a .tar.bz2 with export scripts and a second,
// int8 weight beside it; only `model.onnx` and its licence are kept, under the
// flat name `infra::speech::DIARIZATION_MODEL` reads. The hash of what comes out
// is what holds it, as with the release assets above. MIT, as the archive's
// LICENSE says.
const segmentation = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2";
const pyannoteModel = "220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079";
const pyannoteLicense = "14d7016ad68e7394d6e6b78d96cc2ae431c905287b89674cfdf021e79e62b8ba";

const files: [name: string, url: string, sha256: string][] = [
  // The graph keeps its repo name `model.int8.onnx`; here it is `gigaam.int8.onnx`,
  // flat and self-describing beside the other models, as pyannote's is.
  ["gigaam.int8.onnx", `${gigaam}/model.int8.onnx`, "0f61ec8ca69517cb68e24531c5000053b80ab84cf7071cc294571edfae493462"],
  ["tokens.txt", `${gigaam}/tokens.txt`, "c6a37fab667d53c37e66835c90083808daa116c3993c5e93ce518b5a3896de6e"],
  ["GigaAM-LICENSE", `${gigaam}/LICENSE`, "d02ed0afc408cb4ed5eb34b233096ad8c3db8b6751cd9385276a6725dbf92925"],
  [
    "campplus.onnx",
    `${voices}/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx`,
    "aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2",
  ],
  ["campplus-LICENSE", `${threeDSpeaker}/LICENSE`, "c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4"],
  ["dpdfnet2.onnx", `${denoiser}/dpdfnet2.onnx`, "ce35d6025fc71df0ef10d1540e1b7916837bbfe5f6896deb744508d2cad487a9"],
  ["DPDFNet-LICENSE", `${dpdfnet}/LICENSE`, "c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4"],
  ["silero_vad.onnx", `${sileroVad}/silero_vad.onnx`, "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6"],
  ["Silero-LICENSE", `${silero}/LICENSE`, "2e63e9a38b6e8fc0c7bc37ce174caca1862870856c6daf5697cfb785e925520b"],
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
  // Read whole, not streamed: Bun 1.3.9 hangs writing a response's stream once a
  // large file was read the way `sha256` reads it — the body never comes.
  await Bun.write(partial, await response.arrayBuffer());
  const got = await sha256(partial);
  if (got !== hash) throw new Error(`${name}: sha256 ${got}, expected ${hash}`);
  renameSync(partial, path);
}
// v3's three-file transducer, replaced by v4's single graph: a machine that
// fetched v3 would otherwise bundle its dead weights beside the new model.
for (const stale of ["encoder.int8.onnx", "decoder.onnx", "joiner.onnx"]) rmSync(join(out, stale), { force: true });
const diarization = join(out, "pyannote_segmentation.onnx");
const pyannoteLicence = join(out, "pyannote-LICENSE");
if (!existsSync(diarization) || (await sha256(diarization)) !== pyannoteModel || !existsSync(pyannoteLicence)) {
  console.log("fetching pyannote_segmentation.onnx");
  const archive = join(out, "pyannote.tar.bz2");
  const response = await fetch(segmentation);
  if (!response.ok) throw new Error(`${segmentation}: ${response.status}`);
  await Bun.write(archive, await response.arrayBuffer());
  const unpacked = Bun.spawnSync(["tar", "xjf", archive, "-C", out]);
  if (unpacked.exitCode !== 0) throw new Error(`tar: ${unpacked.stderr.toString()}`);
  const dir = join(out, "sherpa-onnx-pyannote-segmentation-3-0");
  rmSync(diarization, { force: true });
  rmSync(pyannoteLicence, { force: true });
  renameSync(join(dir, "model.onnx"), diarization);
  renameSync(join(dir, "LICENSE"), pyannoteLicence);
  rmSync(dir, { recursive: true, force: true });
  rmSync(archive, { force: true });
  const modelHash = await sha256(diarization);
  if (modelHash !== pyannoteModel) throw new Error(`pyannote_segmentation.onnx: sha256 ${modelHash}, expected ${pyannoteModel}`);
  const licenceHash = await sha256(pyannoteLicence);
  if (licenceHash !== pyannoteLicense) throw new Error(`pyannote-LICENSE: sha256 ${licenceHash}, expected ${pyannoteLicense}`);
}
console.log(`meeting models in ${out}`);
