# Логи из Elasticsearch: инструмент `logsQuery`

План. Решение принято пользователем 2026-09-30: роль Kubernetes в режиме Chat получает
поиск по логам кластеров в Elasticsearch — ES|QL напрямую в ES, с ранбуками по образцу
Kubernetes. Модель — внутренняя, использование — только тестовый стенд; `http://` и
`https://` работают одинаково. Документ фиксирует, что проверено на стенде, решения,
инструмент, ранбуки, нарезку L-1…L-5, бенч и открытые вопросы. Отвечает на открытый
вопрос 7 из [21-kubernetes-mode.md](21-kubernetes-mode.md) («Логи из агрегатора»).

## Зачем

`kubeLogs` читает то, что хранит kubelet: текущий контейнер и один предыдущий. Всё
остальное — там, куда логи увёз Logstash:

- **под, которого уже нет** — удалён, заменён rollout'ом, вытеснен;
- **то, что было час или день назад** — после нескольких рестартов `previous: true` пуст;
- **один запрос через все сервисы** — по `trace_id`, а не под за подом;
- **сколько и с какого момента** — счёт ошибок по приложению и версии, а не хвост одного пода.

Сейчас это делается в Kibana руками, а модель туда не дотягивается.

## Что проверено на стенде (2026-09-30)

Всё ниже — живыми запросами `curl` к тестовому стенду, без кода.

| Что | Результат |
|---|---|
| Версия | Kibana и Elasticsearch 8.19, self-hosted |
| Доступ | ES напрямую по `:9200` (`http://` на тестовом стенде) и через прокси Kibana (`/api/console/proxy`) — оба отвечают |
| API | ES\|QL, `POST /_query` — публичное стабильное API; колонки и строки в ответе |
| Вход | Basic (логин и пароль) работает. Сервер принимает ещё `Bearer` и `ApiKey` (`WWW-Authenticate` в 401) |
| Учётка стенда | `dev`: роли `kibana_user`, `read_all_indices` — читает **все** индексы, но не метаданные кластера (`_cat/indices`, `GET /` — 403) и не создаёт себе ключ |
| Индексы | `kube-logstash-*` — логи Kubernetes, ~540 млн записей в сутки, хранится ~3 суток, переключение раз в сутки (ILM rollover). Рядом `logstash-mesos-*` (Mesos, сотни тысяч в сутки), `mq-cl2-*` (MQ, помесячно), служебные `.monitoring-*`, `.internal.alerts-*` |
| Алиас `logs-*` | На стенде нет — только явный шаблон |
| Схема | Структурированный JSON (logback + logstash-encoder, метаданные Kubernetes): см. ранбук `logs-fields` ниже |
| Скорость | `MATCH(message, "timeout")` + `level == "ERROR"` + `STATS … BY app_id.keyword` за 24 часа по `kube-logstash-*` — **0,46 с** |

Дата в имени индекса — дата переключения, а не записей: в индексе от 29-го лежат записи
30-го. Окно выбирается по `@timestamp` по всему шаблону, не по имени индекса.

Типы ключевых полей (`_field_caps`):

- **`keyword`** — точное `==`, `STATS … BY`: `level`, `logger_name`, `thread_name`,
  `trace_id`, `span_id`, `stakan`, `container_version`, `kubernetes.namespace_name`,
  `kubernetes.pod_name`, `kubernetes.container_name`, `kubernetes.host`,
  `kubernetes.labels.*`;
- **`text` + `.keyword`** — `app_id`: сравнивать и группировать по `app_id.keyword`;
- **только `text`** — `message`, `stack_trace`: только `MATCH` / `QSTR` / `KQL`, без `==`,
  без `STATS BY`, без `LIKE` (на полумиллиарде записей `LIKE` по тексту — полный перебор).

По документации 8.19: `WHERE` с полнотекстовой функцией должен идти **первым после
`FROM`**, и в `OR` её нельзя смешивать с не-полнотекстовым условием; `==` по `text` —
с учётом регистра и по всей строке.

## Почему не MCP

У Elastic два официальных MCP-сервера, и ни один сюда не ложится:

- **`elastic/mcp-server-elasticsearch`** (ES 8.x и 9.x) — **deprecated**, только
  исправления безопасности. Ответы сырые: ни потолка окна, ни урезания полей.
- **MCP-эндпоинт Agent Builder** в Kibana — только Elastic **9.2+**, на 8.19 его нет.

И общее для MCP (как в [24-web-search.md](24-web-search.md)): серверы живут в папке
агента, а у чата папки нет; каждый вызов MCP спрашивает. В режиме Agent устаревший
сервер можно подключить уже сейчас через `mcp.json` — для разовой проверки, не вместо.

## Что уже есть и переиспользуется

| Что | Где | Зачем здесь |
|---|---|---|
| Цикл инструментов роли в Chat | `services/plain_chat.rs`, `llm_chat::Turn` с `Place::Chat` | Ход роли Kubernetes, новый инструмент — просто ещё один |
| Инструмент есть, только когда он настроен | `webSearch`: без ключа нет в запросе, фильтр в `offered()` | Без Settings → Logs `logsQuery` не предлагается |
| HTTP-агент | `infra/http_agent.rs` (`ureq`, свой TLS, `http_status_as_error(false)`) | `http://` и `https://` одним кодом; свой CA для внутреннего сертификата; тело ошибки ES читается, а не теряется |
| Хранилище секретов | `llm_credentials.enc` (`infra/llm_credentials_store.rs`), как ключ Tavily `web-search:tavily` | Пароль или ключ ES, наружу — только `hasSecret` |
| Вычистка credential'ов в тексте | `domain::kube_view::redact_text` | `password=…`, `token: …`, пароль в URL внутри `message` и `stack_trace` |
| Ранбуки | `domain/runbooks.rs`, `infra/runbooks_store.rs`, `kubeRunbook` | Новые ранбуки — файлы рядом с существующими; свой файл заменяет встроенный по имени |
| Таблицы в результате | `domain::kube_view` (таблицы `kubeList`) | Та же форма ответа: колонки, потолок, честная пометка об обрезке |
| Потолок и вычистка старых результатов | `domain/result_clearing.rs`, `domain/loop_guard.rs` | Результаты запросов десятиминутной давности не держат контекст |
| Бенч роли | `services/kube_bench.rs`, `bench/kube-tasks/` | Образец для `bench/logs-tasks/` |

## Решения

### 1. Напрямую в Elasticsearch, язык — ES|QL

- **ES, а не Kibana.** Логи лежат в ES, Kibana — интерфейс над ним. У Kibana 8.19 нет
  публичного API поиска: прокси Dev Tools требует свою привилегию, `/internal/search/*` —
  внутренний и меняется между версиями. Прямой адрес ES на стенде доступен.
- **ES|QL, а не Query DSL.** Запрос — одна строка, которую модель пишет хорошо; фильтр,
  группировка и лимит — в ней же; ответ — таблица, а не вложенный JSON с `_source`.
  Отдельного инструмента для агрегаций не нужно.
- **ES|QL не пишет.** Команд изменения в языке нет, поэтому разбирать запрос ради
  безопасности не нужно — только ради формы ответа (решение 3).

### 2. Инструмент роли Kubernetes, не отдельная роль

Индекс — логи подов Kubernetes, поля `kubernetes.namespace_name` / `pod_name` — те же
имена, что видят `kubeList` и `kubeDiagnose`. Диагностика ходит туда-обратно: под в
CrashLoop → лог упавшего контейнера трёхчасовой давности → версия → откат. Отдельная роль
разорвала бы эту цепочку на два чата.

Роль «Logs» — когда понадобятся логи без Kubernetes (`logstash-mesos-*`, VM); см. «Вне плана».

### 3. `logsQuery`: модель пишет запрос, рамку ставит приложение

Модель пишет ES|QL **после `FROM`**, начиная с `|`. Приложение собирает остальное:

```text
FROM <шаблон из Settings>          ← приложение; свой FROM модели — ошибка с объяснением
<запрос модели>
| KEEP <поля по умолчанию>         ← только если в запросе нет ни KEEP, ни STATS
| LIMIT 200                        ← всегда; LIMIT модели меньше — действует он
```

и тело запроса:

```json
{
  "query": "…",
  "filter": { "bool": { "filter": [
    { "range": { "@timestamp": { "gte": "<since>", "lte": "<until>" } } },
    { "term": { "stakan": "<stakan>" } }
  ] } }
}
```

`POST <адрес>/_query?format=json&drop_null_columns=true`.

- **Окно времени — обязательно и через `filter`, а не в тексте запроса.** Модель не может
  его забыть или написать с ошибкой: `range` накладывается до выполнения ES|QL. Потолок —
  **72 часа** (весь срок хранения на стенде); по умолчанию — последний час. Окно шире
  потолка — ошибка с подсказкой сузить, а не молчаливая обрезка.
- **`stakan`** — необязательный `term` в том же `filter`: у подов в разных кластерах
  бывают одинаковые имена.
- **`LIMIT 200` дописывается всегда.** Два `LIMIT` подряд в ES|QL допустимы, действует
  меньший; без него ES|QL вернул бы до 1000 строк.
- **`KEEP` по умолчанию** — `@timestamp, level, app_id, kubernetes.pod_name, logger_name,
  message, trace_id` — когда в запросе нет ни `KEEP`, ни `STATS`: иначе строка несёт
  десятки колонок, включая метки сетевых политик (`kubernetes.labels.networking_*`).
  Проверка — по слову в тексте, не по разбору ES|QL (`ponytail:` в коде: `KEEP` внутри
  строкового литерала обманет её; разбор — когда это случится на бенче).
- **`drop_null_columns=true`** — колонки, пустые во всех строках, не приходят вовсе.
- **Свой `FROM` модели — ошибка**, а не подмена: «the index is fixed — start with `|`».
  Модель, которой молча переписали запрос, не узнает, что читала не то.

### 4. Форма ответа

Таблица, как у `kubeList`, с заголовком:

```text
kube-logstash-* · 2026-09-30 15:00–16:00 MSK · 20 rows · 0.4 s
@timestamp            level  app_id                   kubernetes.pod_name        message
2026-09-30 15:59:41   ERROR  corp-wowtax-fns-updater  fns-updater-cd7cffd6-58z9r Ошибка в процессе обновления…
```

- **`message`** — до 300 символов в ячейке.
- **`stack_trace`** — при ≤ 3 строках результата первые 40 строк трейса (модель читает
  одну ошибку целиком); при большем числе — только первая строка и «(+N lines)». Полный
  трейс — запрос этой одной записи.
- **Весь результат** — не больше 8 000 символов (как `kube*`, решение 5 в
  [21](21-kubernetes-mode.md)), с честной пометкой: «cut at 8 000 of 31 400 chars — KEEP
  fewer columns or STATS instead of rows».
- **`message` и `stack_trace` проходят `redact_text`** до того, как их увидит модель.
- **`is_partial: true`** — строка «partial: some shards failed», а не тишина.
- **Время** — в часовом поясе машины, как в Kibana.

**Ошибки ES — модели дословно.** 400 от ES|QL несёт «line 2:14: mismatched input…» — по
нему модель чинит запрос сама. Номера строк и колонок — по собранному запросу, поэтому
в ошибку кладётся **и собранный запрос целиком**. 401/403 — «the account cannot read
this — Settings → Logs», таймаут (30 с) — «narrow: add a namespace or an app, shorten the
window». Это `ToolError`, а не `Ok` с текстом ошибки: `loop_guard` считает их как ошибки.

### 5. Подключение: Settings → Logs

Секция в `Settings.tsx`, одна на приложение (не на чат: источник логов один, в отличие от
kubeconfig'ов):

- **Адрес ES** — `http://…` или `https://…`, **одинаково**: один код в
  `infra/elasticsearch.rs`, схема берётся из адреса. Никакой разницы в интерфейсе.
- **Шаблон индекса** — по умолчанию `kube-logstash-*`.
- **Вход — один из двух:** логин и пароль (`Authorization: Basic`) или API-ключ
  (`Authorization: ApiKey`, формат Encoded из Kibana). Секрет — в `llm_credentials.enc`
  под `logs:elasticsearch`, наружу окно узнаёт только, есть ли он.
- **Сертификат CA** (необязательно, для `https://` с внутренним CA) — PEM, через
  `http_agent::build_agent(Some(pem))`, как у провайдеров модели. Переключателя
  «не проверять сертификат» нет: самоподписанный сертификат доверяется явно.
- **Кнопка «Проверить»** — `FROM <шаблон> | STATS n = COUNT(*), newest = MAX(@timestamp)`
  за последний час: «12,4 млн записей за час, последняя — 16:15». Нужен только `read`
  (`GET /` требует `monitor`, которого у `dev` нет). Так же обнаруживается неверный
  пароль или шаблон, под которым ничего нет.
- **Переадресации выключены**, как у MCP по HTTP: заголовок с паролем не уходит на другой хост.

Нет адреса или секрета — инструмента нет в запросе (решение из `webSearch`).

### 6. Данные и безопасность

- **Модель внутренняя, стенд тестовый** — решение пользователя 2026-09-30. Приложение
  этого не проверяет и проверить не может: что вернул `logsQuery`, уходит тому провайдеру,
  который выбран. Это строка в [08-data-policy.md](08-data-policy.md) (L-5).
- **Граница — учётка, а не шаблон.** `FROM` ставит приложение, но ES|QL умеет
  `LOOKUP JOIN` и `ENRICH` к другим индексам, а `dev` читает всё. Настоящая граница — права
  входа: для чего-то, кроме тестового стенда, — ключ с `read` только на
  `kube-logstash-*` (его создаёт админ; `dev` не может).
- **`http://`** передаёт пароль или ключ открытым текстом. Работает так же, как `https://`
  (по решению пользователя); сказано в политике данных, а не в окне.
- **`redact_text`** ловит credential'ы по имени ключа. ФИО, телефоны, номера карт в
  `message` — не ловит. Для тестового стенда с внутренней моделью это принято; до
  чего-то большего — открытый вопрос 3.
- **В лог вызовов** пишутся запрос и окно; строки результата — нет (поле `text`
  вырезается, как у `kube*`).

### 7. Ранбуки для логов

Устроены как у Kubernetes (решение 7 в [21](21-kubernetes-mode.md)): промпт несёт
список «имя — признак», текст модель берёт `kubeRunbook(name)`. Файлы — рядом с
существующими, `src-tauri/runbooks/kubernetes/logs-*.md`, в `BUILT_IN`; свой файл в
`~/.kibo/runbooks/kubernetes/` с тем же именем заменяет встроенный — так другая схема
полей описывается своим `logs-fields.md` без правки кода.

**Ранбук `logs-*` не попадает в список, когда `logsQuery` не предложен** (нет Settings →
Logs): иначе модель прочтёт памятку про инструмент, которого у неё нет.

Кроме новых — по строке в два существующих:

- `crashloop` — шаг 3: «`previous: true` is empty or the pod is gone → runbook `logs-gone-pod`».
- `spring-boot` — «What its errors are»: «older than the pod, or across pods and versions →
  runbook `logs-errors`».

Тексты — для модели, поэтому по-английски, как остальные ранбуки.

#### `logs-fields` — поля и правила запроса

```markdown
# The logs: fields and query rules

Sign: before your first logsQuery in a conversation — which field holds what, and how ES|QL has to be written against it.

The index holds every container's stdout from the Kubernetes clusters, a record per log line, kept about three
days, about 500 million records a day. logsQuery adds FROM and the time window itself: you write the rest,
starting with a pipe.

Who wrote it (keyword: exact ==, STATS BY):
- kubernetes.namespace_name, kubernetes.pod_name, kubernetes.container_name — the names kubeList shows
- app_id.keyword — the application. app_id itself is text: compare and group by app_id.keyword
- container_version — the image version: a record from before a deploy against one from after
- stakan — which cluster; pods in two clusters can share a name (logsQuery takes it as `stakan`)
- kubernetes.host — the node
What it says:
- level (keyword) — ERROR, WARN, INFO, DEBUG, in capitals
- logger_name, thread_name (keyword) — the class and the thread that logged it
- message (text) — the line
- stack_trace (text) — the exception, frames already shortened; absent when there was none
Across services:
- trace_id, span_id (keyword) — one request through every service it touched

Rules:
- Narrow by keyword fields first. Text search over a day is what costs.
- message and stack_trace are text: only MATCH(message, "…"), QSTR("message:\"two words\""), KQL(…).
  Never ==, LIKE or STATS BY on them.
- A WHERE with MATCH/QSTR/KQL has to be the first command of your query, and cannot be OR-ed with a
  condition that is not full-text. Put the keyword conditions in the same WHERE with AND.
- == is case-sensitive: level == "ERROR", not "error".
- Count before you read: STATS n = COUNT(*) BY … answers "how many" and "where"; rows answer "what".
- KEEP what you need. stack_trace only when you read one error, not in a list.
- A timeline: STATS n = COUNT(*) BY t = BUCKET(@timestamp, 10 minutes) | SORT t.
- Nothing found: widen the window before you change the query; records older than three days are gone.
```

#### `logs-errors` — что падает

```markdown
# What is failing

Sign: the user asks which errors there are, why an application or a namespace misbehaves, what broke —
and names no single request.

In order:
1. Where, in one call:
   | WHERE level == "ERROR" AND kubernetes.namespace_name == "…"
   | STATS n = COUNT(*) BY app_id.keyword, logger_name | SORT n DESC | LIMIT 20
   Last hour; nothing there → since 6h. Without a namespace from the user, drop that condition.
2. What the top ones say — rows, not a count:
   | WHERE level == "ERROR" AND app_id.keyword == "…" AND logger_name == "…"
   | KEEP @timestamp, kubernetes.pod_name, message | SORT @timestamp DESC | LIMIT 10
3. One of them in full: the same with KEEP @timestamp, message, stack_trace | LIMIT 1.
   The last "Caused by" is the cause; the frames above it are wrappers.
4. Since when, and was it a deploy:
   | WHERE level == "ERROR" AND app_id.keyword == "…"
   | STATS n = COUNT(*) BY t = BUCKET(@timestamp, 10 minutes), container_version | SORT t
   since 24h. A step up where the version changes → the deploy did it. Steady for a day → an old problem;
   say so rather than presenting it as the incident.
5. One pod or all: STATS n = COUNT(*) BY kubernetes.pod_name. One pod → that pod or its node
   (kubernetes.host); all → the code, the config or a dependency.
6. To the cluster: a pod that exists → kubeDiagnose. A version that brought it → kubeFieldHistory;
   kubeRolloutUndo is a fair offer.

A dependency failing — a refused connection, a timeout, a 4xx/5xx from another host — is not this
application's bug: name the dependency, and check whether other applications fail on it too
(| WHERE MATCH(message, "<host>") | STATS n = COUNT(*) BY app_id.keyword).

Do not: read hundreds of rows to count them — STATS counts. Do not name a cause from message alone when
the record has a stack_trace.
```

#### `logs-trace` — один запрос через сервисы

```markdown
# One request across services

Sign: the user gives a trace id, a request id, an order, document or client number, or asks why one
particular request failed.

1. A trace_id (32 hex characters):
   | WHERE trace_id == "…"
   | KEEP @timestamp, app_id, kubernetes.pod_name, level, logger_name, message
   | SORT @timestamp | LIMIT 200
   since 24h, or the time the user named. That is the whole request, in every service it reached.
2. Any other id — find it in the text first, narrowed by what the user told you:
   | WHERE MATCH(message, "…") AND app_id.keyword == "…"
   | KEEP @timestamp, app_id, trace_id, message | SORT @timestamp | LIMIT 20
   then step 1 with the trace_id found. Records without a trace_id cannot be chained: follow time and pod
   instead, and say so.
3. The first ERROR or WARN in the chain is where it broke; what follows is consequences. Read that one
   record's stack_trace.
4. The chain ends without an error → the next service was never reached or does not log: name the last
   service that logged and what it was calling.

Do not: search a whole day of text across every application when the user named one.
```

#### `logs-gone-pod` — под, которого уже нет

```markdown
# A pod that is gone or restarted

Sign: kubeLogs cannot show what you need — the pod was deleted, replaced by a rollout or evicted,
`previous: true` is empty after several restarts — or the user asks about something hours ago.

1. Every pod of the application, when each lived and which version it ran:
   | WHERE kubernetes.namespace_name == "…" AND app_id.keyword == "…"
   | STATS first = MIN(@timestamp), last = MAX(@timestamp), n = COUNT(*) BY kubernetes.pod_name, container_version
   | SORT last DESC | LIMIT 20
   since 24h.
2. Its last lines:
   | WHERE kubernetes.pod_name == "…"
   | KEEP @timestamp, level, logger_name, message | SORT @timestamp DESC | LIMIT 50
   Read upward: the last ERROR, "Caused by", "OutOfMemoryError", "Shutting down", "SIGTERM".
3. Normal lines, then silence → it was killed from outside (OOMKilled, a failed probe, eviction), and a log
   cannot say which: runbooks oom-killed and probes, kubeEvents while the events are kept (about an hour).
4. No "Started …" line between a pod's first record and its last → it never finished starting: runbook
   probes, or what it waited on (| WHERE kubernetes.pod_name == "…" AND MATCH(message, "Hikari Kafka Flyway
   Liquibase Connection")).

Records are kept about three days. Older than that there is nothing to find — say so.
```

## Инструмент

| Инструмент | Аргументы | Что возвращает |
|---|---|---|
| `logsQuery` | `query` — ES\|QL после `FROM`, начиная с `\|`; `since?` — `30m`, `6h`, `2d` или время ISO (по умолчанию `1h`); `until?` — время ISO (по умолчанию сейчас); `stakan?` | Таблица с заголовком «шаблон · окно · строк · время» (решение 4), или ошибка ES с собранным запросом |

Чтение, без карточки подтверждения. Вариант в `ToolName` и `ToolCall`, модуль
`services/ai_tools/tools/logs.rs`, ветка в `match`; `ChatRole::Kubernetes.tools()` его
перечисляет, `offered()` убирает без Settings → Logs.

**Слои:**

- `domain/logs.rs` — чистое: аргументы, разбор окна и потолок, сборка запроса и `filter`,
  таблица из ответа ES|QL, урезание, ошибки (`thiserror`). Порт —
  `LogsQueryFn = Arc<dyn Fn(&LogsRequest) -> Result<EsqlResponse, LogsError> + Send + Sync>`,
  как `WebSearchFn`.
- `infra/elasticsearch.rs` — единственный сетевой код: `ureq` через `http_agent`, заголовок
  входа, таймаут 30 с, без переадресаций. В `data_policy::NETWORK_ALLOWED`.
- `commands/settings.rs` — `logs_settings_get`, `logs_settings_save`, `logs_check`; обёртки
  в `src/lib/`.

**Промпт роли** — строка в `KUBERNETES_PROMPT`, статичная, как у `webSearch`: «When
logsQuery is among your tools: it reaches logs the kubelet no longer has — pods that are
gone, hours ago, one request across services. Read runbook logs-fields before your first
query.»

## Нарезка

Каждый шаг — отдельный коммит, бэкенд — с мутационной проверкой
([11-mutation-testing.md](11-mutation-testing.md)).

**L-1. Бэкенд инструмента.**
`domain/logs.rs`, `infra/elasticsearch.rs`, `services/ai_tools/tools/logs.rs`; варианты в
`ToolName`, `ToolCall`, `ToolResult`, разбор в `parse.rs`, строка в `DEFINITIONS`, вес,
`ChatRole::tools()`, поле `logs` в `Turn` / `ToolDeps` / `ChatTurn`, фильтр в `offered()`.
`infra/elasticsearch.rs` — в `NETWORK_ALLOWED`.
Тесты: сборка запроса (свой `FROM` — ошибка; `LIMIT` дописан всегда; `KEEP` по умолчанию
только без `KEEP`/`STATS`), окно (`1h` по умолчанию, `2d`, ISO, шире 72 ч — ошибка,
`until` раньше `since` — ошибка), тело `filter` со `stakan` и без, таблица из записанного
ответа 8.19 (формат с `columns`/`values` — со стенда), урезание `message` и `stack_trace` по
числу строк, потолок 8 000, `redact_text` в ячейках, `is_partial`, ошибки 400 (дословно +
собранный запрос), 401/403, таймаут; `http://` и `https://` — один путь (тест на
локальном `TcpListener` без TLS).
Проверки: `cargo check && cargo test`, мутации.

**L-2. Settings → Logs.**
Секция и хук, команды в `generate_handler![]`, обёртки в `src/lib/`. Секрет — в
хранилище под `logs:elasticsearch`; адрес, шаблон, вид входа, CA — в `settings.json`.
«Проверить» — `logs_check`. `commands/plain_chat.rs` собирает `LogsQueryFn`, когда адрес
и секрет есть.
Проверки: `bun run tsc --noEmit && bun test`, `cargo test`.

**L-3. Ранбуки, промпт, вид в чате.**
Четыре файла `runbooks/kubernetes/logs-*.md` в `BUILT_IN`, строки в `crashloop` и
`spring-boot`, фильтр `logs-*` в списке без `logsQuery`, строка в `KUBERNETES_PROMPT`.
`describeTool.ts`: метка «Logs», в развороте — запрос и таблица.
Тест `every_built_in_runbook_has_a_name_a_sign_and_tools_that_exist` уже требует, чтобы
названные инструменты существовали. Прогон `kube_bench`: промпт роли меняется.

**L-4. Бенч и прогон на стенде.**
`bench/logs-tasks/` (ниже). Затем четыре живых вопроса на тестовом стенде — по одному на
ранбук — с внутренней моделью; что модель спросила и где ошиблась в ES|QL — в отчёт шага.

**L-5. Документы.**
Строка «Elasticsearch (логи)» в таблице выходов [08-data-policy.md](08-data-policy.md) и
пункт в «Чего политика не обещает» (граница — учётка; `http://`; вычистка — эвристика),
строка в `docs/README.md`, отметка в открытом вопросе 7 документа 21, пункт в
`CHANGELOG.md`.

Черновик строки в таблицу выходов:

> **Elasticsearch с логами** (режим Chat, роль Kubernetes; [25-logs.md](25-logs.md)) |
> Запрос ES\|QL, который написала модель, с окном времени и шаблоном индекса от
> приложения; логин и пароль или API-ключ — в заголовке `Authorization`, на адрес из
> Settings и никуда больше. Строки логов — `message`, `stack_trace`, имена подов,
> `trace_id` — уходят провайдеру модели как результат инструмента | Пользователь вводит
> адрес и вход в Settings → Logs; без них инструмента нет в запросе | Сетевой код — только
> `infra/elasticsearch.rs` (`NETWORK_ALLOWED`), переадресации выключены. Секрет запечатан в
> `llm_credentials.enc` под `logs:elasticsearch`. `http://` передаёт его открытым текстом —
> работает так же, как `https://`. В лог вызовов пишутся запрос и окно, строки — нет.
> `message` и `stack_trace` проходят `redact_text`: эвристика по имени ключа, не гарантия.
> Что доступно модели, решают права учётки, а не шаблон: `LOOKUP JOIN` и `ENRICH` читают
> другие индексы

## Проверка качества: бенч на локальном ES

По образцу `kube_bench`: `bench/logs-tasks/<сценарий>/` — запрос в словах пользователя и
скрытая проверка ответа. `#[ignore]`, провайдер — `AGENT_BENCH_*`.

- **Elasticsearch** — `docker.elastic.co/elasticsearch/elasticsearch:8.19.x`, один узел,
  в Docker из OrbStack (он уже поднят), security включена с паролем — так проверяется
  Basic, как на стенде. Поднимается один раз на все прогоны.
- **Данные** — генератор в `bench/logs-tasks/`: записи **той же схемы, что на стенде**
  (поля и типы из `_field_caps` выше, шаблон индекса `kube-logstash-*`), время —
  относительно момента загрузки. Фон — десяток приложений с INFO и редкими ERROR; в каждом
  сценарии — своё заложенное происшествие. Загрузка — `_bulk`.

Сценарии — по одному на ранбук:

- **«Что сломалось в namespace `orders` за последний час»** — `payments-api` с версии
  `1.4.2` получает 403 от внешнего адреса, остальное — шум. Проверка: названы приложение,
  403 и адрес, версия как начало.
- **«Почему не прошёл запрос, trace `…`»** — цепочка `gateway → orders → payments`,
  `payments` падает на таймауте к базе. Проверка: названы `payments` и таймаут, а не
  `gateway`, где ошибка всплыла.
- **«Заказ 4711 не оформился»** — номер только в `message` одной записи; от неё —
  `trace_id` и цепочка. Проверка: причина из цепочки.
- **«Под `orders-…-x7k2p` вчера пропал»** — пода нет в кластере, в логах его последние
  строки — `OutOfMemoryError`. Проверка: назван OOM; модель не утверждает, что видела
  событие OOMKilled.

И без модели (`logs_fixtures`, как `kube_fixtures`): эталонная последовательность запросов
каждого сценария находит ответ на загруженных данных — так проверяется сам генератор.

Меряется на прогон: дошла ли до причины; раунды и токены; **запросы с ошибкой ES|QL** —
главная метрика ранбука `logs-fields`: `LIKE` по `text`, `MATCH` не первым, `app_id` вместо
`app_id.keyword`; строк прочитано против строк посчитано `STATS`.

## Вне плана

- **`logstash-mesos-*` и другие шаблоны.** Второй шаблон в Settings и аргумент выбора, или
  роль «Logs» без Kubernetes — когда понадобятся логи не из k8s.
- **Прокси Kibana** (`/api/console/proxy`), если прямой доступ к ES закроют. Отличается
  только адресом и заголовком `kbn-xsrf` — изменение в `infra/elasticsearch.rs`.
- **Ссылка «открыть в Kibana»** на карточке вызова: тот же запрос в Discover.
- **MCP Agent Builder** после обновления до 9.2+ — пересмотреть решение 1.
- **Логи у агента** в режиме Agent — вместе с вопросом 8 документа 21 (кластер у агента).

## Открытые вопросы

1. **`stakan` и контекст kubeconfig.** Модель передаёт `stakan` сама, когда видит
   одинаковые имена подов. Связать контекст закреплённого кластера со `stakan`
   (строка в Settings на контекст) — и фильтр ставило бы приложение. Нужно, если на бенче
   или стенде модель будет путать кластеры.
2. **Учётка вне тестового стенда.** `dev` читает все индексы, включая `mq-cl2-*`. Для
   любого другого стенда — ключ с `read` на `kube-logstash-*` от админа; `dev` создать
   его не может.
3. **Персональные данные в `message`.** `redact_text` ловит только credential'ы по имени
   ключа. Для внутренней модели на тестовом стенде это принято; для продовых логов или
   внешнего провайдера — решать отдельно (маски телефонов и карт? запрет роли без
   внутренней модели?).
4. **Потолки.** 72 часа, 200 строк, 8 000 символов, 300 символов на `message`, 40 строк
   трейса — прикидки. Точные — по бенчу и стенду.
5. **Группировка по тексту.** `STATS … BY` по `message` нельзя. Работает ли
   `EVAL m = SUBSTRING(message, 0, 120) | STATS n = COUNT(*) BY m` на 8.19 и сколько стоит за
   сутки — проверить на стенде; если да, это шаг 2 в `logs-errors` вместо чтения строк.
6. **Второй инструмент `logsFields`** (`_field_caps` без шума) — если модель будет
   спрашивать поля, которых нет в `logs-fields`, или схема разойдётся между стендами.
