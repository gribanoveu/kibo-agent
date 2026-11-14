# PR текущей ветки из Bitbucket Data Center — в панели Changes

План, предложен 2026-11-14. Функция `bitbucket` в Catalog: над списком изменений в панели
Changes — полоска pull request'а текущей ветки с self-hosted Bitbucket Data Center: заголовок,
состояние, ревьюеры и их решения, сборки по HEAD-коммиту, можно ли мержить и почему нет;
кнопки Create PR, Approve / Unapprove, Merge, Open in browser. Образец — то, как Claude
desktop показывает PR с GitHub. Работа кончается тем, что пользователь видит и ведёт свой PR,
не открывая браузер.

Кода в документе нет — только решения.

## Что уже есть

- `infra/repository_identity.rs` — `remote_identity` выбирает remote (`origin` или
  единственный), `canonicalize_remote_url` сводит `ssh://`, `git@host:` и `https://` к
  `host/path`. Разбор координат репозитория начинается отсюда; сейчас обе функции приватные.
- `infra/mongo_store.rs` — список соединений, запечатанный мастер-ключом
  (`secret_store`, `SecretPurpose`), наружу — только вид без секрета (`ConnectionView`).
  Серверы Bitbucket с токенами хранятся так же.
- `infra/http_agent.rs` — `build_agent(trusted_cert_pem)`: `ureq` с `http_status_as_error(false)`
  (тело ошибки читается) и своим доверием к сертификату на агента. Корпоративный CA
  получает только этот сервер, как у AlfaGen (`docs/47-alfagen-provider.md`).
- `domain/features.rs` — манифест функций Catalog; `mongodb` — ближайший образец (данные —
  один `.enc`-файл, «Delete data» его удаляет).
- `components/ChangesPanel.tsx` и `onFolderChanged(root, …)` в `src/lib/git.ts` — панель
  открытой папки и сигнал «папка или `.git` изменились».
- `openExternal` в `src/lib/window.ts` — открыть ссылку в браузере.
- `infra/git_branches::pr_ready` и `offer { kind: "pullRequest" }` (`docs/45-parallel-sessions.md`)
  — агент уже предлагает PR из worktree, но делает его через `runCommand`; к этой функции
  прямого отношения не имеет.
- Опрос в бэкенде с пометкой `ponytail:` уже есть (`services/mcp_servers.rs`), а
  `src/__tests__/architecture.test.ts` запрещает `setInterval` во фронтенде.

Нет: клиента Bitbucket, хранилища серверов, канала событий о PR, UI.

## Решения

### 1. Bitbucket определяется не по адресу, а сервером, которого пользователь добавил

Self-hosted адрес может ничего не говорить: `git.corp.local`, `code.example.ru`. Поэтому
источник истины — список серверов, которые пользователь добавил сам (базовый адрес + токен).
Токен никогда не уходит на хост только потому, что его назвал remote.

Распознавание — только чтобы заполнить форму и подтвердить догадку:

- **Подсказки из remote.** HTTP-клон Data Center выглядит как `https://host[/контекст]/scm/PROJ/repo.git`
  — всё до `/scm/` и есть базовый адрес, вместе с контекстным путём (`/bitbucket`). SSH —
  `ssh://git@host:7999/proj/repo.git`, порт 7999 — порт Bitbucket по умолчанию; базовый адрес
  предлагается как `https://host`.
- **Проба без токена.** `GET {base}/rest/api/latest/application-properties` по документации
  отвечает без аутентификации: JSON с `displayName: "Bitbucket"` и версией. Это и есть
  ответ на «Bitbucket ли это»: ни GitLab, ни Gitea так не отвечают. Проба идёт только по
  кнопке в форме подключения — на адрес, который пользователь видит и может поправить.
- **Принадлежность репозитория — по ссылкам клона.** С токеном
  `GET /rest/api/latest/projects/{p}/repos/{r}` (project и slug — из пути remote) отдаёт
  `links.clone`; один из них, приведённый `canonicalize_remote_url`, обязан совпасть с
  remote. Так проверяется и SSH-хост, который отличается от HTTP-хоста (`git-ssh.corp` против
  `bitbucket.corp`): после совпадения хост remote запоминается у сервера как его псевдоним,
  и следующие репозитории с него находят сервер сами.

Отвергнуто: эвристика по имени хоста (`bitbucket` в адресе) — на self-hosted её нет вовсе;
проба каждого remote автоматически — запрос на любой хост при открытии папки, без ведома
пользователя.

### 2. Только Data Center, API 1.0 (`/rest/api/latest`)

Cloud — другой API (`api.bitbucket.org/2.0`), другая авторизация, другая модель ревьюеров.
Один адаптер под то, чем пользуются. Trait `BitbucketApi` в `domain/` нужен ради тестового
двойника сервиса, а не ради Cloud; Cloud станет второй реализацией, если понадобится.

Запросы этапа 1:

| Что | Запрос |
|---|---|
| PR ветки | `GET …/pull-requests?at=refs/heads/{branch}&direction=OUTGOING&state=OPEN` |
| Можно ли мержить | `GET …/pull-requests/{id}/merge` → `canMerge`, `conflicted`, `vetoes[]` |
| Сборки | `GET …/commits/{sha}/builds`; на старом сервере — устаревший `/rest/build-status/latest/commits/{sha}` (выбор — по версии из `application-properties`, проба BB-0) |
| Approve | `PUT …/pull-requests/{id}/participants/{userSlug}` `{status: APPROVED \| UNAPPROVED}` |
| Merge | `POST …/pull-requests/{id}/merge?version={v}` — стратегия по умолчанию репозитория |
| Create | `POST …/pull-requests` с `fromRef`/`toRef`; цель по умолчанию — default branch репозитория |

`version` у PR — оптимистическая блокировка: merge по устаревшей версии получает 409, и
полоска перечитывается, а не повторяет запрос.

### 3. Токен — HTTP access token (Bearer), запечатан как ключи провайдеров

Personal HTTP access token Data Center, заголовок `Authorization: Bearer`. Права: `REPO_READ`
на чтение и approve, `REPO_WRITE` на create и merge — форма говорит об этом, а 401/403 на
кнопке объясняются этим же текстом. Хранилище — `bitbucket_servers.enc` по образцу
`mongo_store`: окно видит адрес, псевдонимы хостов и «токен задан», но не сам токен.

Отвергнуто: логин и пароль (basic auth) — пароль от SSO в файле; OAuth — нужна регистрация
приложения админом сервера.

### 4. Свежесть: опрос в бэкенде, пока полоска на экране — с пометкой `ponytail:`

`AGENTS.md` запрещает таймер, который переспрашивает бэкенд, — но здесь переспрашивать надо
внешний сервер, а он ничего не скажет сам: вебхуку нужен адрес, до которого Bitbucket
достучится, а у десктопа его нет. Поэтому:

- Опрашивает **бэкенд**, как `infra/background.rs` и `services/mcp_servers.rs`: команда
  `bitbucket_watch(root)` / `bitbucket_unwatch`, пока полоска смонтирована. Раз в 30 с, раз в
  10 с, пока есть сборка `INPROGRESS`. Снимок сравнивается с прошлым, и событие
  `bitbucket:changed { root }` уходит только при разнице — фронтенд перечитывает, как
  везде. `ponytail:` у таймера: потолок — нагрузка на сервер при многих окнах, выход —
  вебхук через relay, если он когда-то появится.
- Сразу, без ожидания тика: при монтировании, при `onFolderChanged` (смена ветки, коммит,
  push меняют `.git`), после любой кнопки и по ручному Refresh.
- Фронтенд не опрашивает ничего — тест `polling` в `architecture.test.ts` остаётся без
  исключения.

### 5. Место — полоска над списком изменений в Changes, а не своя страница

PR — про текущую ветку, а Changes — про то же: что в ветке и что уходит. Отдельная страница
со всеми PR репозитория — другая задача (вне плана). Полоска свёрнута в одну строку
(номер, заголовок, значки сборки и ревью), разворачивается в подробности и кнопки.

Состояния полоски: функция выключена — её нет; repo без remote — нет; remote не сопоставлен
серверу — «Connect Bitbucket» с подсказками из решения 1; ветка не на сервере — «Push the
branch to create a PR» (push сам по себе — вне плана); PR нет — Create PR; PR есть — он;
сервер не отвечает — последний снимок серым и причина.

### 6. Кнопки — только то, что сервер разрешает, и причина, если нет

Approve скрыт у автора (сервер всё равно откажет). Merge неактивен, пока `canMerge` ложно,
и под ним — `vetoes[].summaryMessage` и «conflicts» как есть: это ровно то, что показывает
веб-интерфейс Bitbucket. Merge и Create — через `Modal` с подтверждением (действие уходит на
сервер и не отменяется кнопкой). Open — `openExternal` на `links.self` PR.

## Слои

- `domain/bitbucket.rs` — чистое: `RepoCoords` (хост, project, slug) из канонического remote,
  подсказка базового адреса, выбор сервера по хосту и псевдонимам, `PrSnapshot` (PR, ревьюеры,
  сборки, mergeability, «я — автор»), правила кнопок, разница снимков; `BitbucketError`
  (`thiserror`: нет сервера, не Bitbucket, 401/403 с правами, 409, сеть); trait `BitbucketApi`;
  `BitbucketEventSink` с одним `BitbucketChanged { root }`.
- `infra/bitbucket_store.rs` — серверы под мастер-ключом, `SecretPurpose::BitbucketServers`.
- `infra/bitbucket_client.rs` — `ureq` через `http_agent::build_agent`, реализация `BitbucketApi`;
  текущий пользователь — из ответа сервера (как именно — проба BB-0).
- `services/bitbucket.rs` — снимок для папки, действия, подключение (проба, проверка клона,
  псевдоним), наблюдатель с таймером; наружу — через sink.
- `commands/bitbucket.rs` — `bitbucket_servers`, `bitbucket_server_save/delete`,
  `bitbucket_detect`, `bitbucket_pr`, `bitbucket_create/approve/merge`, `bitbucket_watch/unwatch`;
  всё сетевое — `async` через `commands::blocking`, модуль — в список медленных
  `architecture.rs`; канал `bitbucket:changed` — `pub const` с тестом.
- Фронт: `src/lib/bitbucket.ts` (обёртки и `onBitbucketChanged`), `src/hooks/useBitbucketPr.ts`
  (`useLiveRead` + watch на время жизни), `src/components/PrStrip.tsx` + `.css`,
  `BitbucketConnect.tsx` (форма в `Modal`), список серверов — в Settings.
- Функция `bitbucket` в `domain/features.rs`, `data: Some("bitbucket_servers.enc")`, без
  инструментов агента.

## Нарезка

Каждый этап — отдельный коммит с тестами и мутационной проверкой
([11-mutation-testing.md](11-mutation-testing.md)).

**BB-0. Проба — без кода приложения.** `curl` к серверу пользователя:
`application-properties` без токена (отвечает ли за SSO-прокси, что в `displayName`, какая
версия); Bearer-токен проходит; `pull-requests?at=…&direction=OUTGOING` находит PR ветки;
есть ли `commits/{sha}/builds` (иначе — устаревший путь); где взять slug текущего
пользователя (заголовок `X-AUSERNAME` в ответе или `/plugins/servlet/applinks/whoami`);
нужен ли корпоративный CA; совпадает ли SSH-хост с HTTP-хостом. Ответ «проба за прокси не
отвечает» отменяет автоматическое подтверждение из решения 1 — тогда сервер добавляется
только руками; «CA нужен» — поле PEM у сервера становится обязательным в BB-2.

**BB-1. Домен.** Разбор remote (`/scm/`, контекстный путь, SSH с портом, `~user` —
личные репозитории), подсказка базового адреса, выбор сервера по хосту и псевдониму,
правила кнопок, разница снимков. Тесты: все формы remote из BB-0; remote, не подходящий ни
одному серверу, не выбирает «первый»; автор не может approve; merge закрыт при
`conflicted` или вето; одинаковые снимки — не изменение.

**BB-2. Хранилище и клиент.** `bitbucket_store` по образцу `mongo_store` (токен не уходит в
вид), клиент поверх `http_agent`. Тесты: круг записи и чтения запечатанного списка; разбор
ответов сервера по сохранённым JSON из BB-0; 401/403/409 становятся своими вариантами
`BitbucketError`, а не строкой.

**BB-3. Сервис, команды, событие.** Снимок, действия, подключение с проверкой `links.clone`,
наблюдатель (тик, ускорение при `INPROGRESS`, событие только при разнице, остановка по
unwatch). Тесты на двойнике `BitbucketApi`: снимок собирается из четырёх ответов; merge с 409
перечитывает, а не повторяет; псевдоним хоста запоминается только при совпадении клона;
наблюдатель молчит, пока снимок тот же; после unwatch запросов нет. Имя канала — тест.

**BB-4. Полоска и подключение.** `PrStrip` во всех состояниях решения 5, `BitbucketConnect`,
серверы в Settings, функция в Catalog. Тесты (`bun test`): каждое состояние рисует своё;
Merge неактивен с причиной; подтверждение перед Merge и Create; событие чужого `root` не
перечитывает.

**BB-5. Документы.** `docs/README.md`, `CHANGELOG.md`, `08-data-policy.md` (что уходит на
сервер Bitbucket и когда: проба без токена — по кнопке; с токеном — только на добавленный
сервер).

## Вне плана

- **Bitbucket Cloud** — когда понадобится: вторая реализация `BitbucketApi`.
- **Push из полоски** — сейчас push делает пользователь или агент через `runCommand`; если
  «Push the branch» окажется частым шагом, кнопка пойдёт через `git_cli` отдельным этапом.
- **Список всех PR репозитория, комментарии, дифф PR, inline-комментарии в коде** — своя
  страница, своя фича.
- **Инструмент агента** (`bitbucketPr`: прочитать PR и сборки, создать PR) — когда агент
  должен сам смотреть на CI; тогда и `offer { kind: "pullRequest" }` создаёт PR через этот
  клиент, а не через `runCommand`.
- **Decline, выбор стратегии merge, Needs work** — веб-интерфейс одним кликом из Open.

## Открытые вопросы

1. Корпоративный CA: сервер Bitbucket внутренний, как AlfaGen? Тогда — поле PEM у сервера,
   с готовым вариантом «тот же корень, что у AlfaGen», или только поле.
2. Push — оставить вне плана, как выше, или кнопка Push в полоске нужна сразу?
3. Нужен ли агенту доступ к PR уже в этом PR (читающий инструмент), или позже?
