# GigaAM v4: распознавание речи точнее почти в полтора раза

План, предложен 2026-11-28. Заменяем GigaAM v3 (RNN-T с пунктуацией, экспорт csukuangfj)
на GigaAM v4 CTC-E2E int8 (экспорт ai-babai) в meeting notes и диктовке. Тот же рантайм
sherpa-onnx (бамп 2.0.2 → 2.0.3, согласован), тот же второй процесс (`meeting_sidecar` /
`--dictation-sidecar`); файлы модели проще — один граф + tokens вместо
encoder/decoder/joiner. Работа заканчивается PR в main: модель v4 в бандле, загрузчик
NeMo CTC, обновлённые доки, live-тесты диктовки/записи прогнаны.

Кода в документе нет — только решения.

## Внешние факты (проверены 2026-11-28)

- v4 вышла 2026-11-27, MIT, Zapformer 238M, RU+EN. WER RU (карточка модели):
  CTC 6.33% против 9.51% у v3 RNN-T; Callcenter 7.20% против 11.04% — наш сценарий созвонов.
- Экспорт: `ai-babai/gigaam-v4-onnx`, ревизия `d39e2a200f596315acef7003d246b65a73e9a327`,
  каталог `ctc-e2e/`:
  - `model.int8.onnx` — 242.1 МБ, sha256 `0f61ec8ca69517cb68e24531c5000053b80ab84cf7071cc294571edfae493462`;
  - `tokens.txt` — sha256 `c6a37fab667d53c37e66835c90083808daa116c3993c5e93ce518b5a3896de6e`;
  - `LICENSE` (MIT) — хэш берётся из `SHA256SUMS` той же ревизии;
  - `preprocessor.onnx` не нужен — sherpa считает mel сам.
- Официальная страница sherpa-onnx
  (<https://k2-fsa.github.io/sherpa/onnx/pretrained_models/offline-ctc/nemo/gigaam-v4.html>):
  грузится штатным NeMo CTC-загрузчиком, `--feature-dim=64`, проверено на sherpa-onnx 2.0.3
  (32/32 RU/EN клипов на macOS/Linux, 4/4 Windows).
- Крейт `sherpa-onnx` 2.0.2 (`src-tauri/Cargo.toml`): `nemo_ctc` в API есть.
  Latest — 2.0.3 (2026-11-22). Фикс таймстампов CTC (k2-fsa/sherpa-onnx PR #4018)
  смержен 3 октября — входит в обе версии.
- Размер: v3 ≈ 232 МБ (224.6 + 4.6 + 2.7) → v4 ≈ 242 МБ (+10 МБ, ~4%).
  int4-вариант 152 МБ есть, но точность автором не замерена.
- Таймстампы у offline CTC — по токенам (`timestamps.size() == tokens.size()`);
  `durations` и так TDT-only, у нас `Option`.

## Что уже есть

- `src-tauri/src/infra/speech.rs` — весь пайплайн: `MODEL_FILES` (6 файлов), `has_models`,
  `DICTATION_FILES` (5), `dictation_models` (Silero = `DICTATION_FILES[4]`), `recognizer`
  (nemo_transducer), `transcribe` (окна 24 с / перекрытие 4 с — не трогаем),
  campplus = `MODEL_FILES[4]`, dpdfnet = `MODEL_FILES[5]`. Тесты на состав бандла — рядом.
- `src-tauri/src/domain/meeting.rs` — `words_of` уже склеивает SentencePiece-токены
  (`▁` начинает слово, пунктуация остаётся с предыдущим) — для CTC-E2E подходит без правки.
  `TOKEN_SECONDS = 0.08` предполагает кадр 40 мс — проба сверяет шаг таймстампов v4.
- `scripts/fetch-meeting-models.ts` — пиннинг commit + sha256, докачка только
  изменившегося; меняются константа `gigaam` и первые 5 строк массива `files`.
- `src-tauri/src/dictation_sidecar.rs` — тест пишет `DICTATION_FILES[..4]` (файлы GigaAM).
- `src-tauri/src/services/meeting_notes.rs` — тест пишет все `MODEL_FILES`
  (переживёт смену списка сам).
- `experiments/dictation-probe/src/main.rs` — свой `recognizer()` с тем же
  transducer-конфигом; обновить вместе с основным.
- `src-tauri/tauri.conf.json` — ресурс `models/meeting/` целиком, менять не нужно.
- Диктовка использует тот же распознаватель: `Phrases(OfflineRecognizer)` — один вызов
  `recognize` на фразу; фразовая логика (Silero, пауза 0.6 с, перераспознавание открытой
  фразы, тон «слушаю») от модели не зависит. Live-тест `dictation_live` покрывает сценарий
  целиком (CLAUDE.md).

## Решения

### 1. CTC-E2E int8, не RNN-T

RNN-T экспорт v4 — свой encoder+advance граф с SentencePiece-токенизаторами, грузится
только «Sherpa preview», не нашим крейтом. CTC грузится штатным загрузчиком, WER 6.33%
против 5.99% — разница мала против выигрыша к v3. Отвергнуто: свой рантайм на `ort`
ради RNN-T.

### 2. int8, не int4

int4 на 90 МБ меньше, но точность не замерена автором; +10 МБ к бандлу — приемлемо.
Вернёмся к int4, если появится замер или потребность.

### 3. Крейт 2.0.3

Согласовано пользователем 2026-11-28 (минорная версия). Это версия, на которой экспорт
проверен автором и в доках sherpa; `sherpa-onnx-sys` подтянется тем же бампом (build.rs
качает прособранную статическую библиотеку — механизм не меняется).

### 4. Экспорт стороннего автора, пиннинг как у csukuangfj

Ревизия + sha256 в скрипте, MIT LICENSE рядом с весами. Доверие держит хэш, не имя автора.

### 5. Имена файлов плоские и говорящие

`gigaam.int8.onnx` (переименование при скачивании, как у pyannote) + `tokens.txt`;
`GigaAM-LICENSE` остаётся. `MODEL_FILES` 6→4: `gigaam.int8.onnx`, `tokens.txt`,
`campplus.onnx`, `dpdfnet2.onnx`; `DICTATION_FILES` 5→3: два файла GigaAM + `silero_vad.onnx`.
Индексы campplus (4→2), dpdfnet (5→3), Silero (4→2) сдвигаются.

### 6. VAD и диаризация не трогаются

Silero остаётся (GigaAM VAD без ONNX/sherpa-поддержки), pyannote segmentation остаётся
(GigaAM VAD — не диаризация). Окна 24 с не трогаются (лимит CTC ~30 с на фразу — укладываемся).

### 7. feature_dim=64 — проверяется пробой

Docs-пример передаёт явно; наш конфиг не задаёт. Если метаданные графа не подхватываются —
задать `feat_config.feature_dim = 64` в `recognizer()`.

## Нарезка

Каждый этап — отдельный коммит с тестами и мутационной проверкой
([11-mutation-testing.md](11-mutation-testing.md)).

**GV-0. Проба — без кода приложения.** Скачать `ctc-e2e` int8 pinned-ревизии в scratchpad.
Rust-пробник (по образцу `experiments/dictation-probe`) на крейте 2.0.3: загрузка через
`nemo_ctc`; RU и EN клипы; текст с пунктуацией; таймстампы по токенам, их шаг и склейка
`words_of`; окно 24 с; RTF и время загрузки на 4 потоках (сравнить с v3: RTF 0.06–0.07,
загрузка ~0.6 с по docs/52); нужен ли явный feature_dim=64; короткие фразы 1–3 с
(режим диктовки). Провал загрузки отменяет решение 1 (пересмотр с пользователем).

**GV-1. Переключение модели.** `Cargo.toml`: sherpa-onnx 2.0.3 (+`Cargo.lock`);
`scripts/fetch-meeting-models.ts` (новые URL/хеши/LICENSE, 2 файла GigaAM); `speech.rs`:
`MODEL_FILES`→4, `DICTATION_FILES`→3, `recognizer()` на `nemo_ctc` (+feature_dim по пробе),
индексы campplus/dpdfnet/Silero, комментарии; тесты состава бандла; recognizer в
`experiments/dictation-probe`. Тесты обязаны покрыть: состав `MODEL_FILES`/`DICTATION_FILES`
(каждый файл назван), `has_models` отклоняет неполный набор, `recognizer` смотрит на новые
имена. Проверки: `cargo check && cargo test`, `bun run tsc --noEmit && bun test`;
live: `dictation_live`, `record_live`, `denoise_live`. Что не прогнано — в отчёт этапа.

**GV-2. Документы и финал.** `docs/36-meeting-notes-research.md` и `docs/52-voice-input.md`
(v3→v4, цифры WER/размер, источник экспорта), строки в `docs/README.md`, `CHANGELOG.md`
под Unreleased, статус плана. Затем финал: merge main, проверки заново, PR.

## Вне плана

- Стриминговый v4 для диктовки (partial-текст во время речи): нужен свой рантайм поверх
  step-графов или нативная поддержка Zapformer в sherpa-onnx. Вернёмся, когда sherpa поддержит.
- GigaAM VAD вместо Silero: нет официального ONNX и поддержки в sherpa; Silero подобран
  и закреплён тестами. Вернёмся при официальном экспорте.
- int4-квантизация: без замера точности не берём.
- Диаризация, окна нарезки, денойзер — не трогаются.

## Открытые вопросы

1. ~~Бамп `sherpa-onnx` 2.0.2→2.0.3~~ — **решено 2026-11-28**: пользователь согласовал,
   версия минорная. Бамп в GV-1, проба GV-0 на 2.0.3.
2. ~~Материал для сравнения WER v3/v4 на наших записях~~ — **решено 2026-11-28**:
   пользователь предоставит запись созвона по запросу; когда она понадобится —
   остановиться и попросить. Проба GV-0 идёт на test_wavs, сравнение на записи —
   отдельный шаг GV-1.