# Ядро без Tauri: крейт `kibo-core`

План, предложен 2026-11-19. Вынести `domain/`, `services/` и `infra/` из крейта
`kibo-agent` в отдельный крейт `kibo-core` того же Cargo workspace, без зависимости от
Tauri. Десктоп остаётся как есть — тот же exe, тот же бандл, те же команды. Ради чего:
второй фронт к тому же ядру (облачный `kibo-server`: команды поверх HTTP/WebSocket, под на
пользователя) становится вторым крейтом-соседом, а не форком; граница «ядро не знает про
UI» держится компилятором, а не только тестом над исходниками. Работа кончается, когда
`cargo build -p kibo-core` собирается без `tauri` в графе, а приложение и все тесты — как
до неё.

Кода в документе нет — только решения.

## Что уже есть

Проверено по `main` (`2033b265`).

| Что | Где | Что важно для плана |
|---|---|---|
| Один крейт | `src-tauri/Cargo.toml`, `kibo-agent`, lib `kibo_agent_lib` (`staticlib`, `cdylib`, `rlib`) | Workspace нет, cargo-фич нет ни одной |
| Размер слоёв | `src-tauri/src/` | `commands/` — 34 файла, 7,3 тыс. строк; `services/` + `domain/` + `infra/` — 246 файлов, 98 тыс. строк. Тестов: 2330 в ядре, 162 в `commands/` и корневых модулях |
| Ядро → `commands` | — | Ни одной ссылки `crate::commands` из `services/`, `domain/`, `infra/` |
| Tauri ниже `commands/` | 4 файла `infra/` | `meeting_audio.rs`, `prototype_protocol.rs` — `tauri::http::Response`, а это реэкспорт крейта `http` (`pub use http;` в `tauri`), который уже в зависимостях; `kube_client.rs`, `mcp_rmcp.rs` — `tauri::async_runtime::block_on`/`spawn` |
| Tauri-cfg | — | `cfg(desktop)`/`cfg(mobile)` в ядре не встречается; `debug_assertions` — в `prototype_protocol.rs` и `speech.rs`, это cfg компилятора, переносится как есть |
| Общие корневые модули | `sync.rs` (`lock`, 9 строк) | Им пользуются 8 файлов `infra/` и `services/` — уходит в ядро |
| Тестовая опора | `testing.rs` (`#[cfg(test)]`), `infra::app_dir` | `app_dir::dir()` под `cfg(test)` подменяется временной папкой, чтобы тест не писал в настоящий `~/.kibo`. Пользуются ею и тесты ядра, и тесты `commands/` (`agent_state`, `approval`, `chat`, `settings`…) |
| Пути к ассетам | `env!("CARGO_MANIFEST_DIR")` в `infra/local_embeddings.rs`, `infra/speech.rs`, `infra/prototype_protocol.rs`, `services/dictation.rs`, `services/*_bench.rs` | Запасной путь разработки: `models/`, `prototype-libs/`, `bench/`, `Cargo.lock`. В сборке путь приходит через `resource_dir` — шов уже есть |
| Сайдкары | `meeting_sidecar.rs`, `dictation_sidecar.rs`, `main.rs` | Второй процесс того же exe; берут из ядра только `domain` и `infra::speech` |
| Проверки слоёв | `architecture.rs`, `data_policy.rs` | Тесты над деревом `src/` (`crate_dir().join("src")`); правило для `services/` сейчас разрешает `tauri::async_runtime` |
| `build.rs` | `src-tauri/build.rs` | Читает `generate_handler![]` из `lib.rs` — остаётся с десктопом |
| CI | `.github/workflows/test.yml` | `cargo test --locked` в `src-tauri`, macOS и Windows; Linux нет. `rust-cache` — `workspaces: src-tauri -> ../cargo-build` |
| Мутации | `scripts/mutate.py` | План указывает `cwd: src-tauri` и пути файлов `src-tauri/src/...` |
| Нативное в ядре | `infra/microphone.rs`, `meeting_recorder.rs` (`cpal`), `recording_access.rs` (`objc2`, `winreg`), `speech.rs` (`sherpa-onnx`), `master_key.rs` (`keyring`), `terminal.rs` (`portable-pty`), `kube_client.rs`, `mongo_client.rs` | Сервисы зависят от них напрямую: `meeting_notes` → `meeting_recorder`, `dictation` → `microphone`, `turn_setup` → `kube_client`. Перенести их в крейт десктопа нельзя — ядро зависело бы от десктопа |

## Решения

### 1. Workspace с корнем в `src-tauri`, ядро — `src-tauri/core`

`src-tauri/Cargo.toml` остаётся пакетом `kibo-agent` и становится корнем workspace;
`kibo-core` — член в `src-tauri/core/`. `Cargo.lock`, `target/`, `tauri.conf.json`,
ассеты и `build.rs` не двигаются, поэтому Tauri CLI, кеш CI (`workspaces: src-tauri`),
`scripts/clean-build-cache.sh` и команды из `CLAUDE.md` (`cd src-tauri && cargo test …`)
работают без правок. `default-members` — оба крейта, иначе `cargo test` в корне
непустого workspace гоняет только корневой пакет и 2330 тестов ядра молча выпадают.

Отвергнуто: виртуальный workspace в корне репозитория с `crates/kibo-core` и
`crates/kibo-desktop` — переезжают `Cargo.lock`, `target`, ключ кеша CI и все пути в
документах, а выигрыша для ядра никакого.

### 2. Ядро — один крейт, а не три

`domain`, `services`, `infra` уходят вместе, со своими модулями как есть. Направление
зависимостей между ними уже держит `architecture.rs`; три крейта превратили бы каждое
`pub(crate)` на стыке в `pub` и утроили бы манифесты ради того, что тест уже ловит.
Вернёмся, если сборка ядра станет узким местом и замер покажет выигрыш от деления.

### 3. Десктоп реэкспортирует модули ядра

В `lib.rs` десктопа — `pub use kibo_core::{domain, services, infra, sync}`. Тогда
`crate::services::…` в `commands/`, сайдкарах и `lib.rs` продолжает разрешаться, и
перенос не трогает 34 файла команд. Отвергнуто: заменить все пути на `kibo_core::…` —
тот же результат ценой диффа на каждый файл и конфликтов со всеми открытыми ветками.

### 4. Перенос — `git mv` отдельным коммитом

Сначала коммит, который только перемещает файлы, затем — коммит, который чинит
видимость и манифесты. Так `git log --follow` и `blame` идут сквозь перенос, а ревьюер
видит в правках только правки. Открытые ветки при мерже `main` получают переименования,
которые git сводит сам.

### 5. Своя среда tokio у ядра, Tauri ходит в неё же

`kube_client` и `mcp_rmcp` вместо `tauri::async_runtime` берут среду из ядра (один
`tokio::runtime::Runtime` на процесс, создаётся при первом обращении). Десктоп при
старте отдаёт её ручку Tauri через `tauri::async_runtime::set`, поэтому в процессе
остаётся одна среда, а не две. В графе уже есть tokio с `rt-multi-thread` (его тянет
Tauri), новой зависимости нет — у ядра только добавляются фичи `rt-multi-thread` в своём
манифесте. Отвергнуто: оставить ядру зависимость от `tauri` ради `async_runtime` — тогда
`kibo-server` тянул бы Tauri со всем вебвью.

### 6. `tauri::http` → `http`

Это тот же крейт `http` 1.x, поэтому `meeting_audio` и `prototype_protocol` меняют только
`use`; обработчики протоколов `artifact://` и `meeting://` в `lib.rs` получают тот же тип.

### 7. Тестовая опора ядра — фича `test-support`

`testing.rs` и подмена `app_dir::dir()` живут в ядре под
`cfg(any(test, feature = "test-support"))`. Десктоп включает фичу только в
`[dev-dependencies]`. Без этого тесты `commands/` собирали бы ядро без `cfg(test)`, и
`app_dir::dir()` вёл бы в настоящий `~/.kibo` разработчика — тест молча переписал бы его
настройки и ключи. Резолвер 2 (edition 2021) не протаскивает фичу dev-зависимости в
обычную сборку, так что в бандл она не попадает; это проверяется тестом (KC-2).

### 8. Ассеты остаются в `src-tauri/`, ядро ищет их через одну константу

`models/`, `prototype-libs/`, `bench/`, `postman-sandbox/` — ресурсы бандла, их путь знает
`tauri.conf.json`, и переносить их незачем. В ядре все `env!("CARGO_MANIFEST_DIR")`
заменяются одной константой «папка ассетов разработки» (на уровень выше манифеста ядра).
Рабочий путь по-прежнему приходит через `resource_dir`.

### 9. Нативное остаётся в ядре, отключение — не здесь

`cpal`, `sherpa-onnx`, `keyring`, `kube`, `mongodb` переезжают в ядро как есть: сервисы
зависят от них напрямую, и разрезать их фичами можно только вертикалью целиком (домен,
сервис, infra, команды, страница, строка в `domain/features.rs`). Какие вертикали
отключать и как — решается вместе с сервером, по его нуждам, а не заранее.

### 10. Проверки слоёв сканируют оба крейта

`architecture.rs` и `data_policy.rs` остаются в десктопе (им нужен `lib.rs` с
`generate_handler!` и `src/lib/` фронта), а сканируют `src/` и `core/src/`. Правило для
`services/` теряет исключение `tauri::async_runtime`, и появляется новое: в манифесте
ядра нет ни одной зависимости `tauri*`.

## Нарезка

Каждый этап — отдельный коммит (или пара: перенос и правки) с тестами и мутационной
проверкой ([11-mutation-testing.md](11-mutation-testing.md)) там, где появляется логика.

**KC-0. Проба — собирается ли ядро на Linux.** Без кода приложения: `cargo check` нынешнего
крейта в Linux-контейнере (OrbStack, образ `rust`, с `pkg-config`, `libdbus-1-dev`,
`libasound2-dev`). Ответ: что из `keyring`/`cpal`/`sherpa-onnx`/записи созвона не
собирается и какие системные пакеты нужны. Если собирается — KC-4 сводится к одному job
в CI; если нет — список того, что нужно закрыть `cfg`, уходит в «Открытые вопросы» и
план сервера. KC-1…KC-3 от пробы не зависят.

**KC-1. Развязка с Tauri внутри одного крейта.** Ещё до переноса: `tauri::http` → `http`;
своя среда tokio в ядре и `tauri::async_runtime::set` при старте; `sync.rs` и
`testing.rs` готовы уехать (`testing` — под `cfg(any(test, feature = "test-support"))`,
фича объявлена, пока пустая); одна константа папки ассетов вместо `CARGO_MANIFEST_DIR`;
правило `services/` в `architecture.rs` без исключения для Tauri, такое же правило для
`infra/`. Тесты: всё как было (число тестов записать до и после), среда tokio отдаёт одну
и ту же ручку, `block_on` с потока `spawn_blocking` работает. Живые: `live_cluster`
(`kube_client`), MCP по HTTP — на том, что есть локально.

**KC-2. Workspace и перенос.** `[workspace]` с `members = ["core"]` и
`default-members`; `git mv` `domain/`, `services/`, `infra/`, `sync.rs`, `testing.rs` в
`core/src/` — отдельным коммитом; затем манифест ядра (зависимости, которые нужны ему,
уходят из десктопа; лицензия GPL-3.0-only — из-за `domain::command_risk`), реэкспорт в
`lib.rs` десктопа, `pub(crate)` → `pub` там, где команды берут из ядра (61 место в
ядре — кандидаты, список даёт компилятор), `test-support` в dev-зависимостях, проверки
слоёв на оба дерева. Тесты: число пройденных и `#[ignore]`-тестов (`cargo test -- --list`,
`-- --ignored --list`) совпадает с KC-1; новый тест — манифест ядра без `tauri*`;
сборка без dev-зависимостей не включает `test-support` (`cargo tree -e features`
в тесте или в CI). Замер: чистая сборка и инкрементальная после правки в `commands/` —
до и после.

**KC-3. Пути в инструментах и документах.** `scripts/mutate.py` (пример плана),
`scripts/embedding-model.md`, 10 ссылок `src-tauri/src/{services,domain,infra}` в
`docs/`, раздел «Architecture» в `AGENTS.md`, `CLAUDE.md` (команды остаются, меняются
пути файлов). Проверка: прогон `mutate.py` с одной мутацией в `core/src/...`;
`bun run tauri build` на macOS собирает бандл, приложение открывает папку, индекс,
чат с инструментом, запись созвона и диктовку (сайдкары).

**KC-4. Linux в CI.** По ответу KC-0: job на `ubuntu-latest` с `cargo check -p kibo-core
--locked` и нужными пакетами. Тесты ядра на Linux — нет: им нужны LFS-модели и минуты CI,
а цель job — не дать ядру потерять сборку на платформе сервера.

**KC-5. Документы.** `docs/README.md` (статус), `docs/03-architecture.md` (крейты и где
проходит граница), `AGENTS.md`. `CHANGELOG.md` — без строки: пользователь ничего не видит.

## Вне плана

- **`kibo-server`** — команды поверх HTTP/WebSocket, транспорт в `src/lib/`, вход через
  Keycloak, под на пользователя. Отдельный план после этого.
- **Отключение нативных вертикалей фичами** (запись созвона, диктовка, Kubernetes,
  MongoDB, связка ключей) — решается в плане сервера, см. решение 9.
- **Деление ядра на `domain`/`services`/`infra` крейтами** — см. решение 2; вернёмся по
  замеру времени сборки.
- **Перенос ассетов в ядро** — см. решение 8; вернёмся, если серверу понадобятся модели
  без каталога `src-tauri`.

## Открытые вопросы

1. Linux-job в CI (KC-4) — сейчас или вместе с планом сервера? Это минуты CI на каждый PR
   ради платформы, на которой сегодня никто не запускает приложение.
2. Лицензия ядра остаётся GPL-3.0-only (`domain::command_risk` на правилах sh-guard).
   Для десктопа ничего не меняется; если сервер когда-нибудь будет раздаваться бинарником
   третьим лицам, это вопрос к юристу до плана сервера, а не к этому плану.
