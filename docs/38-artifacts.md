# Артефакты: встречи и прототипы в одном месте, как папки на Диске

План, принят 2026-10-22; сделано AR-1 (формат). Новая страница сайдбара, **Artifacts**: всё, что
приложение сделало для пользователя, разложено по папкам, как на Google Диске. Верхний
уровень — тип артефакта (`Meetings`, `Prototypes`), в нём — папка на каждую встречу или
прототип под её названием, внутри — файлы. Файл открывается справа, в панели страницы:
заметки как Markdown, запись — плеером, страница прототипа — в том же изолированном
iframe, что и в чате.

```text
Artifacts
├── Meetings
│   ├── Планёрка по релизу 2.4        notes.md · transcript.md · audio.wav
│   └── Интервью с кандидатом         …
└── Prototypes
    ├── Лендинг кофейни               index.html · app.jsx · assets/logo.png
    └── …
```

## Что уже есть

| Что | Где | Как пригодится |
|---|---|---|
| Встреча — папка `~/.kibo/meetings/<uuid>/`: `meeting.json` (транскрипт, имена, заметки, название) и `audio.wav` | `infra/meeting_store.rs` | Коллекция `Meetings` — эти папки как есть, `list()` уже отдаёт строку на встречу |
| Прототип — папка `~/.kibo/prototypes/<chat id>/`: `files/` (рабочая), `versions/<n>/` (снимки `present`) | `infra/prototype_store.rs` | Коллекция `Prototypes` — эти папки; файлы — содержимое `files/` |
| `artifact://<chat>/files/<path>` отдаёт файлы прототипа с CSP, `meeting://localhost/<id>` — запись встречи | `infra/prototype_protocol.rs`, `infra/meeting_audio.rs` | Превью HTML, картинок и звука без новых протоколов; CSP окна уже пускает `artifact:` в `img-src`/`frame-src` и `meeting:` в `media-src` |
| Страница сайдбара с панелью справа | `components/pages.tsx`, `Page`/`PagePanel` в `Page.tsx` | Новая страница — одна запись в `PAGES` |
| `Markdown`, `highlight`/`languageOf`, `Dropdown`, `Modal` | `components/Markdown.tsx`, `lib/highlight.ts` | Просмотр заметок и кода, меню и подтверждения |
| Удаление встречи вместе с голосами из неё | `services::meeting_notes::delete_meeting` | Удаление коллекции `Meetings` — тот же вызов |
| Удаление чата вместе с папкой прототипа, с открытым чатом на экране | `services::plain_chat::delete_chat`; `plain.remove(id)` в `App.tsx` | Удаление прототипа — тот же путь |
| Сигналы об изменениях: `meeting:event` (`transcribed`, `ended`), `chat:turn-event` (`toolResult` с `changes` или результатом `present`) | `lib/meeting.ts`, `lib/chat.ts` | Перечитать список — без нового канала (решение 5) |
| «Сохранить как…» прототипа одним HTML, «Показать в Finder» | `infra/prototype_export.rs`, `prototype_reveal` | Те же действия в меню файла |

## Формат встреч: пригоден, менять не нужно

Проверено по `meeting_store.rs` и `domain/meeting.rs` на `40f14f4`. Встреча — уже ровно то,
что нужно папке на Диске:

- **Одна папка — одна встреча, и всё её — в ней.** `meeting.json` и `audio.wav` рядом,
  удаление берёт папку целиком (`delete`), посторонний id до файлов не доходит (`ours`).
- **Название есть и хранится в самой папке:** `title` в `meeting.json`, пока заметок нет —
  `name` (имя файла записи); `meetingTitle` во фронтенде уже это и делает.
- **Дата и размер есть:** `created` (для записанного звонка — начало записи, `1dda22d`),
  `bytes` записи — в `MeetingInfo`.
- **Недоделанное не видно:** папка без `meeting.json` (распознаётся сейчас или брошена) не
  листается, `recordings/` — тоже. Так же и в артефактах: необработанные записи — дело
  страницы Meetings, а не архива готового.

Чего в папке нет — заметок и транскрипта **отдельными файлами**: оба живут внутри
`meeting.json`. Писать рядом `notes.md` и `transcript.md` не предлагается: переименование
спикера (`Meeting::rename`) и новые заметки (`take_notes`) правят JSON, и вторую копию
пришлось бы держать в согласии с ним при каждой правке — два источника правды ради
показа. Вместо этого сервис артефактов **выводит** файлы из встречи при чтении
(решение 2): `notes.md` — `# title` и `summary`, `transcript.md` — `render_transcript`,
`audio.wav` — настоящий файл. Сам `meeting.json` не показывается: это хранилище, не
документ.

Единственная правка у встреч — не в формате, а в правиле: название, данное
пользователем, не перезаписывается заметками (решение 6).

## Формат прототипов: нужен манифест `prototype.json`

Папка прототипа на диске о себе ничего не говорит. Её имя — id чата, а всё, чем прототип
назван и что в нём главное, лежит только в транскрипте чата в `chats.db`: название и
страница каждой версии — аргументы вызовов `present`, и `usePrototypePreview` собирает
версии именно оттуда. Для страницы артефактов это значит: чтобы назвать папку, пришлось
бы разбирать тело каждого чата (`body`, формат `domain::chat_record` — контракт хранилища
чатов, а не прототипов), а чтобы открыть прототип — угадывать, какой HTML в нём страница.

Поэтому — один маленький файл рядом с `files/` и `versions/`:

```text
<app dir>/prototypes/<chat id>/
├── prototype.json        ← новое
├── files/
└── versions/<n>/
```

```json
{
  "title": "Лендинг кофейни",
  "titleByUser": false,
  "page": "index.html",
  "presented": "2026-10-22T10:15:00Z",
  "version": 3
}
```

- **Пишет его `present`** (`services/ai_tools/tools/present.rs`) после `prototype_store::kept`:
  название и страница вызова, время, номер версии. `present` идёт внутри хода, который
  держит `prototype_versions::claim` папки, поэтому писатели не пересекаются; запись —
  `app_dir::write_private` (временный файл и rename), файл целый или прежний.
- **Название пользователя держится.** `titleByUser: true` (после переименования на
  странице артефактов) — `present` обновляет `page`, `presented`, `version`, но не `title`.
- **Вне `files/`**, как `versions/`: модель его не видит и не перепишет, в снимки он не
  попадает, `same_as_version` его не сравнивает.
- **Только последняя версия.** Список версий с названиями остаётся у транскрипта и панели
  Preview в чате; странице артефактов нужна одна папка — что сейчас и что открывать.
  Дублировать историю в манифест — та же вторая правда, что и с `notes.md` у встреч.
- **Прототип виден, когда он показан.** Папка `files/` создаётся на первом ходе любого
  чата с ролью Prototype, в том числе пустого. В артефакты попадает папка, у которой есть
  хотя бы одна версия — так же, как встреча попадает с `meeting.json`.
- **Старые папки — без миграции.** Прототипы, показанные до манифеста, берут название
  у своего чата (`COALESCE(generated_title, title)` из `chats.db`, новая
  `chat_store::title(id)` — один запрос по ключу), страницу — `index.html` из последней
  версии, если она есть, время — mtime последней версии. Ничего обратно не пишется:
  следующий `present` в этом чате напишет манифест сам. Чат удалён, а папка осталась
  (не должно случаться, `delete_chat` удаляет обе) — название «Untitled prototype».
- **Режимы по требованию (`docs/35`)** формат не меняют: папка остаётся на чат, кто бы ни
  вызвал `present` — роль Prototype или Assistant с включённым режимом.

## Решения

### 1. Модель: тип → коллекция → файлы, в `domain/artifacts.rs`

```rust
/// Верхняя папка страницы. Порядок вариантов — порядок папок.
pub enum ArtifactKind { Meetings, Prototypes }

/// Папка одного артефакта: встреча или прототип.
pub struct Collection {
    pub kind: ArtifactKind,
    /// Id встречи или чата — то, чем её знает свой store.
    pub id: String,
    pub title: String,
    /// RFC 3339: когда встреча распознана / прототип последний раз показан.
    pub updated: String,
    pub bytes: u64,
    pub files: usize,
}

pub struct ArtifactFile {
    /// Внутри коллекции, через `/`: `assets/logo.png`.
    pub path: String,
    pub bytes: u64,
    pub view: FileView,
}

/// Чем открывается файл — решает домен по расширению, фронтенд только рисует.
pub enum FileView { Markdown, Code, Html, Image, Audio, Other }

#[derive(thiserror::Error)]
pub enum ArtifactError {
    #[error("no such artifact")] NotFound,
    #[error("{0} is too big to show here")] TooBig(String),
    #[error(transparent)] Meeting(#[from] MeetingError),
    #[error(transparent)] Prototype(#[from] PrototypeError),
    #[error(transparent)] Chat(#[from] ChatError),
}
```

Чистые правила в домене — `FileView::of(path)`, порядок (коллекции новые сверху, файлы —
папки, затем по имени), чтобы фронтенд не дублировал решения. Вложенность внутри
коллекции — только через `path`: у прототипа есть `assets/`, у встречи подпапок нет.
Дерево из путей строит фронтенд: уровень — это файлы с общим префиксом пути.

**Трейта «источник артефактов» нет.** Источников два, оба конкретные и в одном
процессе; сервис делает `match kind`. Третий тип артефакта — третий вариант
`ArtifactKind`, и компилятор покажет все `match`, которые его ждут. Трейт — когда
появится источник, которого не перечислить в `match` (расширения, `docs/16`).

### 2. Сервис `services/artifacts.rs`

- `list() -> Vec<Collection>` — все коллекции обоих типов: `meeting_store::list()`
  (уже отдаёт название, дату, размер записи) и обход `prototypes/` с манифестами.
  Тип `Meetings` — только пока фича `meeting-notes` включена (`Features`, как у
  страниц): выключенная фича не светит своих данных и здесь.
- `files(kind, id) -> Vec<ArtifactFile>` — у встречи выведенные `notes.md` (если есть
  `summary`), `transcript.md`, `audio.wav` (если есть); у прототипа — дерево `files/`
  без ссылок, как его копирует снимок.
- `read(kind, id, path) -> String` — текст для `Markdown`/`Code`: у встречи — собранный
  из `meeting.json`, у прототипа — файл из `files/`, путь проверен тем же правилом, что
  у `artifact://` (`prototype_protocol::file`), предел 1 МБ (`TooBig`). HTML, картинки и
  звук через команду не ходят — их читают протоколы.
- `rename(kind, id, title)` и `delete(kind, id)` — решение 6.

`ponytail:` `list` читает каждый `meeting.json` целиком, как и `meeting_store::list` —
индекс, когда встреч будут сотни.

### 3. Команды и обёртки

`commands/artifacts.rs`, все `async` через `commands::blocking` (диск, SQLite):

| Команда | Обёртка в `src/lib/artifacts.ts` |
|---|---|
| `artifact_list()` | `artifactList(): Promise<Collection[]>` |
| `artifact_files(kind, id)` | `artifactFiles(kind, id)` |
| `artifact_read(kind, id, path)` | `artifactRead(kind, id, path)` |
| `artifact_rename(kind, id, title)` | `artifactRename(kind, id, title)` |
| `artifact_delete(kind, id)` — только встречи, решение 6 | `artifactDelete(kind, id)` |
| `artifact_reveal(kind, id)` | `artifactReveal(kind, id)` |
| `artifact_save_as(kind, id, path, dest)` — копия файла коллекции; страница прототипа — одним HTML через `infra/prototype_export.rs` | `artifactSaveAs(kind, id, path)`: путь из `pickSavePath` (`lib/dialog.ts`), отмена — ничего не делает |

Адреса для протоколов — чистые функции в той же обёртке: `artifactUrl(chatId, path)`
рядом с готовым `audioUrl(id)` из `lib/meeting.ts` (с учётом `http://*.localhost` на
Windows). Регистрация — `generate_handler![]` в `lib.rs`; главное окно получает команды
само (`build.rs`), окну просмотрщика они не нужны. Модуль — в список «медленных» в
`architecture.rs`, если тест этого попросит.

### 4. Страница и её место

Запись в `PAGES` (`components/pages.tsx`), после Meetings: `{ id: "artifacts", label:
"Artifacts", title: "Artifacts — meetings and prototypes, as folders", icon: FolderOpen,
dock: false }`, без `feature` — страница есть всегда, пустой тип просто не показан.

`ArtifactsPage.tsx` + `ArtifactsPage.css`, состояние — в `hooks/useArtifacts.ts` (где мы,
что выбрано, список), не в `App.tsx`:

- **Вверху `Page`** — хлебные крошки `Artifacts › Meetings › Планёрка…`, каждая
  кликабельна, и поле фильтра по названию (на клиенте: список и так целиком).
- **Список, а не плитки:** строки с иконкой, названием, датой и размером — как в Диске в
  режиме списка. Корень — две строки-папки со счётчиком; тип — коллекции, новые сверху;
  коллекция — файлы. Двойной клик или `Enter` — открыть, `Backspace` — на уровень вверх;
  обе клавиши — записи в `lib/shortcuts.ts`, проверяются `matches(e, id)`.
- **Меню строки** (`Dropdown` у кнопки `⋯`): Rename, Delete, Show in Finder, а у
  коллекции ещё «Open in Meetings» / «Open chat».
- Цвета, отступы, шрифт — токены `tokens.css`; иконки — lucide (`Folder`, `FileText`,
  `FileCode`, `Image`, `AudioLines`).

`PageContext` получает два поля из `App.tsx`: `openChat(id)` (выбрать чат и уйти со
страницы) и `removeChat(id): Promise<boolean>` — тот же `plain.remove`, что у сайдбара.
«Open in Meetings» — `onPage("meetings")` с id встречи; для этого `MeetingsPage` берёт
необязательный начальный id. Других правок в `App.tsx` нет.

### 5. Перечитывание — по сигналам, которые уже есть

Нового канала нет: всё, что меняет артефакты, уже о себе сообщает.

- **Встреча готова или заметки переписаны** — `meeting:event` `transcribed` и `ended`
  (задача переживает страницу Meetings и идёт, пока открыта страница артефактов).
- **Прототип показан или его файлы правлены** — `chat:turn-event` `toolResult` с
  непустыми `changes` или результатом `present`. Ход идёт под страницей: чат остаётся
  смонтирован. `onTurnEvent` фильтрует по `turnId`, которого странице не знать, поэтому
  в `lib/chat.ts` — `onAnyToolResult(handler)` над тем же каналом.
- В `lib/artifacts.ts` — `onArtifactsChanged(handler)`, соединяющий оба, как
  `onFolderChanged` соединяет индекс и git. Страница читает через
  `useLiveRead(artifactList, onArtifactsChanged, [], visible)`: подписка, пока она на
  экране, одно чтение при подписке, сигнал — перечитать. Открытая коллекция перечитывает
  свои `files` тем же сигналом.
- Удаление и переименование со страницы перечитывают после своего вызова. Удаление чата
  из сайдбара и откат версии делаются в чате, когда этой страницы на экране нет, — их
  покажет чтение при следующем открытии.

Поток `toolResult` невелик (вызов инструмента — не дельта текста), троттлить нечего.

### 6. Открытие, переименование, удаление

**Открытие — в `PagePanel` справа**, по `FileView`:

| `view` | Как |
|---|---|
| `Markdown` | `artifactRead` → `Markdown` |
| `Code` | `artifactRead` → `highlight(languageOf(path))`, только чтение |
| `Html` | iframe на `artifact://<chat>/files/<path>` с тем же `sandbox`, что в `PrototypePreview`; сам iframe выносится из него в `PrototypeFrame.tsx`, чтобы правила изоляции были в одном месте; кнопка «Развернуть» на всю страницу |
| `Image` | `<img src=artifact://…>` |
| `Audio` | `<audio controls src=meeting://…>` |
| `Other` | имя, размер, «Show in Finder» |

Открыть коллекцию прототипа целиком (двойной клик по папке с зажатым `⌥` или кнопка
«Open» в панели) — страница из манифеста (`page`). Старые версии — в чате, в Preview:
«Open chat».

**Переименование — у коллекции, не у файла.**

- Встреча: новый `Meeting.title_by_user: bool` (`serde(default)`), `rename` ставит
  `title` и флаг; `take_notes` не заменяет название с флагом. Формат файла совместим:
  старые встречи читаются с `false`. Через `meeting_notes`, а не напрямую в store, — там
  же, где переименование спикера.
- Прототип: `title` и `titleByUser: true` в `prototype.json`, под `claim` папки. Идёт ход
  в этом чате — `Busy`, страница говорит «Chat is working — try when it is done».
- **Файлы не переименовываются и не удаляются.** У встречи они выведены из одной записи.
  У прототипа это рабочая папка модели: `index.html`, переименованный снаружи, ломает
  страницу и то, что модель помнит о папке, — менять её файлы должен ход, а откатывать —
  Restore. Картинки в `assets/` удаляются там, где добавляются, — в Preview чата.

**Удаление — коллекция целиком, всегда через `Modal`.**

- Встреча — `meeting_notes::delete_meeting`: папка с записью и то, что голосовая память
  услышала в ней. Встреча, которую сейчас обрабатывают, — отказ с причиной: в
  `delete_meeting` сейчас такой проверки нет, её держит только страница Meetings, —
  с артефактами она нужна в сервисе.
- Прототип живёт, пока жив чат (`docs/31`, решение 1), и отдельно от чата не удаляется:
  папка без чата — сирота, а чат без папки — карточки версий, которые не открываются.
  Delete у прототипа — удаление чата через `removeChat` из `PageContext`, и `Modal`
  говорит это прямо: «Deletes the chat “…” with its prototype». Открытый чат
  `plain.remove` уже отпускает или отказывает, как у сайдбара.
- Корзины нет — как и у встреч сейчас; если понадобится, это `trash/` под app dir и
  перенос папки, не новая модель.

## Нарезка

- **AR-1 · формат.** `prototype.json`: тип в `domain/prototype.rs`, чтение/запись в
  `prototype_store`, запись из `present`, `titleByUser`; `Meeting.title_by_user` и правило
  в `take_notes`; `chat_store::title`. Тесты: манифест после `present`, название
  пользователя переживает `present` и новые заметки, папка без версий не листается.
- **AR-2 · домен и сервис.** `domain/artifacts.rs` (`FileView::of`, порядок, ошибки),
  `services/artifacts.rs` (`list`, `files`, `read`), выведенные файлы встречи, старый
  прототип без манифеста. Тесты на `with_app_dir`.
- **AR-3 · команды и обёртки.** `commands/artifacts.rs`, `generate_handler![]`,
  `lib/artifacts.ts` с `artifactUrl`, `onAnyToolResult`, `onArtifactsChanged`.
- **AR-4 · страница.** `ArtifactsPage`, `useArtifacts`, запись в `PAGES`, крошки, список,
  фильтр, клавиши в `shortcuts.ts`, панель просмотра; `PrototypeFrame` из
  `PrototypePreview`. Тесты Bun: навигация по уровням, перечитывание по сигналу, что
  открывает каждый `view`.
- **AR-5 · действия.** `rename`, `delete`, `reveal`, `openChat`/`removeChat` в
  `PageContext`, «Open in Meetings», `Save as…` у любого файла (заметки `.md`, запись
  `.wav`, файл прототипа; страница прототипа — одним HTML). Тест: `dest` вне выбора
  диалога не принимается, `path` не выходит из коллекции. Строка в `CHANGELOG.md`.

Мутационная проверка (`docs/11`) — по AR-1 и AR-2: правило `titleByUser`, «папка без
версий не видна», путь в `read` не выходит из `files/`.

**AR-1 сделано 2026-10-22.** `domain::prototype::Manifest` (`Manifest::presented` —
правило `titleByUser`), `prototype_store::{manifest, write_manifest, presented}`, запись
из `present`; `Meeting.title_by_user` и его правило в `take_notes`; `chat_store::title`.
Мутаций 12, поймано 12: потерянная метка и её значение у обоих типов, `serde(default)` у
старых файлов, `present` без прежнего манифеста, папка без версии, половинная копия и
чужое имя в списке, нечитаемый манифест.

## Вне плана

- Версии прототипа папками (`versions/1/…`) на странице артефактов — история остаётся у
  чата.
- Необработанные записи (`meetings/recordings/`) — их место на странице Meetings.
- Загрузка своих файлов, перенос между коллекциями, общий доступ — это уже хранилище, а
  не обзор сделанного.
- Поиск по содержимому (заметкам, коду прототипов). Фильтр — только по названию.
- Плитки с миниатюрами страниц: снимок iframe — отдельная задача и цена.

## Решённые вопросы

1. **Страница в сайдбаре — после Meetings**: она собирает сделанное, а не начинает
   работу.
2. **Тип `Meetings` при выключенной фиче не показывается**, даже если встречи на диске
   есть: «Delete data» в каталоге и так их убирает, а выключенная фича — выключенная
   везде.
3. **Delete у прототипа удаляет чат**: одно понятное действие и никаких сирот.
4. **`Save as…` — у любого файла**, а не только у страницы прототипа: копия по
   системному диалогу, в AR-5.
