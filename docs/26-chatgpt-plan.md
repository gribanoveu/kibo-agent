# Провайдер ChatGPT: работа в лимитах подписки

План (не начат). Решение принято пользователем 2026-10-01: добавить провайдера, который
входит через аккаунт ChatGPT и тратит лимиты его плана Plus или Pro вместо API-ключа.
Документ описывает, что OpenAI разрешает и как это устроено, почему OAuth пишем сами,
что переиспользуется, решения, нарезку C-0…C-5 и открытые вопросы.

## Зачем и почему только OpenAI

С API-ключом пользователь платит за каждый токен отдельно, хотя у него, скорее всего, уже
есть подписка. 29 сентября 2026 OpenAI открыла «Sign in with ChatGPT» с доступом к лимитам
плана: open-source и локальные приложения подключают его **без одобрения**, платные и
серверные — через форму заявки. Kibo распространяется под GPL-3.0 и работает локально,
поэтому попадает в первую группу.

У Anthropic такого нет. В документации Agent SDK прямо сказано, что без предварительного
одобрения сторонние приложения не могут предлагать вход через claude.ai и лимиты
подписки. Провайдер `anthropic` остаётся провайдером с ключом.

## Что говорит OpenAI

Сверено по developers.openai.com/siwc 2026-10-01: Overview, Registration and sign-in,
Models and inference, Profiles and sessions, Token reference, Errors and recovery,
Preview limitations, UI/UX guidelines. Функция в превью, поэтому перед каждым шагом
документацию сверяем заново.

### Вход

1. Приложение один раз генерирует `ext_agent_host_id` (`urn:uuid:…`) и хранит его.
   Это непрозрачный идентификатор установки, а не секрет.
2. Слушатель запускается **до** открытия браузера: `http://127.0.0.1:{port}/callback`.
   Хост `127.0.0.1` (не `localhost`), путь `/callback`; от попытки к попытке меняется
   только порт.
3. Браузер открывает `https://auth.openai.com/api/accounts/authorize` с параметрами:
   - `client_id=dynamic_agent_client` при первой регистрации; при повторном входе —
     ранее выданный id;
   - `agent_name_hint=Kibo Agent` — только при первой регистрации;
   - `ext_agent_host_id`, `response_type=code`, `redirect_uri`;
   - `scope=openid profile email offline_access resource.invoke chatgpt.tokens.use.direct`;
   - `resource=https://api.openai.com/v1`;
   - `state`, `nonce`, `code_challenge` (S256), `code_challenge_method=S256`;
   - необязательные `id_token_hint` и `login_hint`.
4. В ответ приходит `?code=…&state=…&scope=…&client_id=oaiapp_…`. **Выданный `client_id`
   приходит здесь**, и сохранять нужно его, а не `dynamic_agent_client`. При
   `error=access_denied` проверяем `state` и останавливаемся, код не обмениваем.
5. Обмен кода: form POST на `https://auth.openai.com/api/accounts/oauth/token` с полями
   `grant_type=authorization_code`, выданный `client_id`, `code`, `code_verifier`,
   `redirect_uri`, `resource`. Секрет клиента не нужен. В ответе: `access_token`,
   `refresh_token`, `id_token`, `expires_in`, `scope`, `earliest_refresh_at`.
6. Проверки: у ID-токена `iss`, `aud` (= выданный `client_id`), `exp` и `nonce`.
   Учётную запись идентифицирует `sub`. В выданных правах должен быть
   `chatgpt.tokens.use.direct`.

### Токены

- Токен доступа живёт **1 час**, токен обновления — **30 дней**. Каждое обновление выдаёт
  новый токен обновления, тоже на 30 дней.
- Обновление: form POST на тот же адрес с полями `grant_type=refresh_token`, `client_id`,
  `refresh_token` и `resource`. Сохраняется всегда последний выданный токен.
- **Обновления одной сессии идут строго по очереди.** Если два обновления запустить
  одновременно, токен обновления израсходуется дважды (`refresh_token_reused`), и сессия
  пропадёт.
- Выход: POST на `revocation_endpoint` с `refresh_token` и `client_id`. Если сеть
  недоступна, токены всё равно стираем и сообщаем, что отзыв на сервере не подтверждён.
- Хранение: защищённое локальное хранилище. Никогда в URL, логах или хранилище окна.
- Несколько аккаунтов — это несколько регистраций, у каждой свои токены.

### Запросы

- `POST https://api.openai.com/v1/responses` с `Authorization: Bearer <access_token>`.
  Обязательно `store: false` и `stream: true`; `input` — массив.
- **Не поддерживаются:** `previous_response_id` (историю каждый раз шлём целиком),
  `max_output_tokens`, `temperature`, `top_p`, `truncation`, `prompt_cache_retention`,
  `metadata`, `user`, `background`, `conversation`. Элементы
  `{type:"message", role:"system"}` отклоняются: системный промпт передаётся в
  `instructions` или сообщением с ролью `developer`.
- **Инструменты.** Дословно сказано: «group function/custom tools in namespaces or supply
  them through `additional_tools` input items». Работает ли обычное поле `tools` с
  `type: "function"`, не сказано — это проверяет C-0. `tool_search` не работает.
- Картинки и файлы во входе поддерживаются, если их принимает модель. Про `reasoning` и
  `include: ["reasoning.encrypted_content"]` документация молчит — тоже вопрос для C-0.
- Модели: `GET https://api.openai.com/v1/models` с тем же токеном. Показываем те, у
  которых `visibility == "list"`: на экран идёт `display_name`, в запрос — `slug`.
- Ход успешен только после события `response.completed`. Ошибка лимита приходит событием
  `response.failed`.

### Ошибки

| Код | HTTP | Что делать |
|---|---|---|
| `subscription_sharing_usage_limit_exceeded` | 429 | Остановиться и показать «Manage usage». **Не повторять**: лимит недельный |
| `subscription_sharing_user_not_eligible` | 403 | План не подходит. Не повторять |
| `subscription_sharing_unsupported_capability` | 400 | В запросе поле или модель, которые не поддерживаются. Не повторять |
| `subscription_sharing_invalid_user` | 401 | Войти заново |
| `subscription_sharing_usage_unavailable` | 503 | Повторить с паузой |
| `invalid_grant`, `invalid_refresh_token`, `token_expired`, `refresh_token_invalidated`, `refresh_token_reused` при обновлении | — | Стереть токены, войти заново |

### Интерфейс (UI/UX guidelines)

- Кнопка входа «Continue with ChatGPT» с белым логотипом ChatGPT.
- Рядом с выбором модели, пока активен этот провайдер, — пометка «Using ChatGPT plan».
- При **первом** входе — окно «You're using your ChatGPT plan». Только один раз.
- Ссылка «Manage usage» ведёт в ChatGPT → Settings → Usage. Когда лимит исчерпан, это
  главное действие.

## OAuth: готовые библиотеки и почему пишем сами

| Вариант | Что даёт | Почему нет |
|---|---|---|
| `oauth2` 5.0 (Rust) | PKCE, сборка ссылки, разбор ответа с токенами | Из коробки умеет только `reqwest` или `ureq` 2, а у нас `ureq` 3. `reqwest` прямой зависимостью запрещает `data_policy.rs` (`RUST_DENIED`). Обходится своим `SyncHttpClient` поверх `ureq` 3 и `http` 1, но всё нестандартное здесь — `dynamic_agent_client`, `client_id` из адреса возврата, `agent_name_hint`, `ext_agent_host_id`, `resource` в обоих запросах — всё равно пишется руками через `add_extra_param`. Экономит строк 50 ценой нового крейта |
| `openidconnect` 4 | Проверка ID-токена по JWKS, discovery | Тяжёлый, а подпись проверять не нужно: токен получен напрямую от token endpoint по TLS (OIDC Core 3.1.3.7, п. 6) — достаточно проверить `iss`, `aud`, `exp` и `nonce` |
| `jsonwebtoken` | Проверка подписи RS256 | Пригодится, только если решим проверять подпись (вопрос 3). `ring` уже в графе |
| `rmcp`, фича `auth` | OAuth для MCP | Привязан к MCP: обнаружение метаданных и DCR по спецификации MCP. Тянет `oauth2` и `reqwest` |
| `tauri-plugin-oauth` | Временный сервер на localhost | Адрес с кодом уходит в окно (`onUrl` в JS), хотя обмениваем код всё равно в Rust. Слушатель на `std::net::TcpListener` — около 40 строк, и план 18 уже выбрал его |
| `@siwc/local`, `@siwc/react` (devkit OpenAI) | Готовые вход, хранилище, кнопки и карточка подключения | `@siwc/local` — это Node-рантайм, а Node в приложении нет. Лицензия devkit — **Noncommercial v1.0**, несовместимая с GPL-3.0: распространять такое в составе Kibo нельзя |
| JS-библиотеки в окне (`oauth4webapi`, `openid-client`) | Весь поток во фронтенде | `fetch` из окна запрещают CSP и `data_policy.rs` (`WINDOW_NETWORK`), а токены не должны попадать в окно |

**Решение: пишем в Rust на том, что уже есть в графе.** Нужны `sha2` (уже в
`Cargo.lock`), `rand`, `uuid`, `url`/`form_urlencoded` (в графе), `ureq` и `serde_json`.
base64url без паддинга — около 15 строк, либо прямая зависимость `base64` (0.21 уже в
графе транзитивно). **Новых крейтов ноль.** Фронтенд только вызывает команды и рисует
состояние. PKCE и слушатель пишутся без привязки к ChatGPT, чтобы шаги F-7.4h/i плана
[18](18-mcp-oauth.md) потом взяли их готовыми.

## Что уже есть и переиспользуется

| Что | Где | Зачем здесь |
|---|---|---|
| Трейт провайдера и фабрика | `domain::llm::LlmProvider`, `infra/llm_providers/mod.rs::provider_for` | Новый `ProviderKind::ChatGpt` — ветка в `match` и модуль рядом с `anthropic.rs` |
| Поток строк SSE с отменой | `llm_providers::StreamLines` | Чтение событий Responses API тем же способом |
| `native_content` в `LlmMessage` | `domain/llm.rs` | Элементы `reasoning` (если C-0 покажет `encrypted_content`) возвращаются в следующий раунд дословно, как thinking у Anthropic |
| Повтор при лимите и 5xx | `domain::llm_retry` (`RateLimited`, `Unavailable`) | `usage_unavailable` → `Unavailable`; **`usage_limit_exceeded` сюда не попадает** (решение 4) |
| Зашифрованное хранилище | `infra/secret_store.rs`, `master_key`, `app_dir` | Новое назначение `SecretPurpose::ChatGptSession`, файл `chatgpt_sessions.enc` |
| HTTP-агент | `infra/http_agent.rs::build_agent` | Запросы и к `auth.openai.com`, и к `api.openai.com` |
| Системный браузер | `tauri-plugin-opener` | Ссылка входа и «Manage usage» |
| Сессия на ход | `services::llm_session::resolve` | Провайдер собирается заново на каждый ход и берёт токен у источника, а не строку ключа |
| Форма провайдера | `src/components/ProviderSettings.tsx` (сегменты `KINDS`) | Третий вид — ChatGPT, без полей адреса и ключа |
| Диалоги | `src/components/Modal.tsx` | Приветственное окно и окно «лимит исчерпан» |

## Решения

### 1. Один провайдер — один аккаунт ChatGPT

Запись в `providers` с `kind: "chatGpt"`. Адрес фиксированный и в форме не показывается.
Сессия (выданный `client_id`, `sub`, email, токены, срок действия) хранится в
`chatgpt_sessions.enc` под `id` провайдера. Второй аккаунт — это второй провайдер, так
требование OpenAI о нескольких профилях ложится на уже существующий список. Общий
`ext_agent_host_id` хранится в `app_dir` в `chatgpt-host.json`, открытым текстом.

### 2. Провайдер берёт источник токена, а не ключ

`provider_for` для `ChatGpt` вместо `SecretString` получает
`Arc<dyn Fn() -> Result<SecretString, LlmError> + Send + Sync>`. Источник отдаёт текущий
токен доступа и обновляет его, если до истечения меньше 2 минут. Ход агента может длиться
дольше часа, а провайдер живёт весь ход, поэтому токен, взятый один раз в начале, к концу
хода истечёт. На `401` источник обновляет токен один раз, и запрос повторяется. `401`
означает, что запрос не выполнялся, так что правило «упавший вызов не повторяется» не
нарушается.

Обновления идут через один `Mutex` на сессию в `infra`, и одновременно выполняется только
одно. Помощники (`domain::agents`) работают параллельно и могут упереться в обновление
одновременно; без блокировки это ровно `refresh_token_reused`.

### 3. Протокол — отдельный модуль `infra/llm_providers/chatgpt.rs`

Отдельный модуль, а не флаг в `openai_compatible.rs`: у Responses API другие и запрос, и
поток.

- **Запрос.** `System` → `instructions`. `User` и `Assistant` → элементы `message`.
  Вызовы → `function_call` (`call_id`, `name`, `arguments`), результаты →
  `function_call_output`. Форма инструментов — по итогам C-0. Не отправляются
  `temperature`, `top_p` и `max_tokens` из `ProviderConfig`: сервер их отвергает, и форма
  их для этого вида не показывает.
- **Поток.**
  - `response.output_text.delta` → `on_delta`;
  - `response.reasoning_summary_text.delta` → `on_reasoning`;
  - `response.function_call_arguments.delta` → `on_tool_call_delta` с накопленными
    аргументами;
  - `response.completed` → `usage`: `input_tokens`, `output_tokens`,
    `input_tokens_details.cached_tokens`;
  - `response.incomplete` → `truncated`;
  - `response.failed` → ошибка по таблице выше.
- **Модели** — `/v1/models`, фильтр `visibility == "list"`, в `LlmModelInfo.id` — `slug`.

### 4. Исчерпанный лимит — не `RateLimited`

`RateLimited` повторяется автоматически (`domain::llm_retry`). На недельном лимите это
бесполезные запросы и задержка до того, как пользователь увидит, в чём дело. Нужен
отдельный вариант `LlmError::PlanLimit`: без повтора, а в UI — окно с «Manage usage».
`invalid_user`, `invalid_grant` и подобные дают `LlmError::SignInRequired`: провайдер
показывает «Войти снова», и ход заканчивается понятным сообщением.

### 5. Вход — только из настроек, не посреди хода

Как в плане 18: ход не ждёт браузер. Команды:

- `chatgpt_sign_in(providerId)` — асинхронная, `spawn_blocking`, таймаут 5 минут, можно
  отменить;
- `chatgpt_sign_in_cancel`;
- `chatgpt_sign_out(providerId)` — отзывает токен и стирает сессию;
- `chatgpt_status(providerId)` — подключён или нет, email, нужен ли повторный вход.

Браузер открывает слой команд; сервис получает порт `open_url: Fn(&str)`, без `AppHandle`.
Слушатель: `127.0.0.1:0`, одна попытка, только путь `/callback`, сверка `state`, в ответ
короткая страница «можно закрыть вкладку».

### 6. Политика данных

`infra/chatgpt_auth.rs` добавляется в `NETWORK_ALLOWED`. В `08-data-policy.md` появляется
новый адрес: `auth.openai.com` (вход, обновление, отзыв) — туда уходят только OAuth-поля.
Запросы к модели уходят в `api.openai.com` так же, как к любому провайдеру. Токены не
попадают ни в лог чата, ни в debug-лог провайдера, ни в окно.

## Нарезка

### C-0 — проба на живом аккаунте

Тест с `#[ignore]` (по образцу `live_cluster`) или одноразовый бинарь в scratchpad: вход,
затем запросы. Нужен аккаунт пользователя с Plus или Pro, а войти в браузере он должен
сам. Проверяем:

- верхнеуровневое `tools` с `type: "function"` — принимается или требует namespace /
  `additional_tools`;
- два раунда с `function_call` и `function_call_output` без `previous_response_id`;
- `reasoning.effort` и `include: ["reasoning.encrypted_content"]` — принимаются ли и
  нужно ли возвращать элементы `reasoning`;
- роль `developer` против `instructions`;
- список моделей и их `slug`;
- `earliest_refresh_at` и само обновление;
- адрес `revocation_endpoint`: есть ли
  `https://auth.openai.com/.well-known/openid-configuration`.

Результаты записываются сюда, и после этого решение 3 фиксируется. **Без C-0 дальше не
идём:** от формы инструментов зависит, работает ли агент вообще.

### C-1 — чистая логика (`domain/chatgpt_auth.rs`, `domain/pkce.rs`)

- PKCE (verifier, S256), `state`, `nonce`;
- сборка ссылки авторизации для первого и повторного входа;
- разбор адреса возврата: `code`, `state`, `client_id`, `error`;
- проверка полей ID-токена (payload в base64url, без подписи);
- решение «пора обновлять»;
- разбор кодов ошибок → `ChatGptAuthError` (thiserror).

Тесты: вектор RFC 7636; чужой `state`; `dynamic_agent_client` никогда не сохраняется как
выданный id; просроченный и чужой `aud`.

### C-2 — сеть и хранение (`infra/chatgpt_auth.rs`, `infra/chatgpt_session_store.rs`)

- Слушатель, обмен кода, обновление, отзыв — на `ureq`.
- Хранилище на `secret_store`; `chatgpt-host.json`.
- Источник токена с `Mutex` на сессию.

Тесты на подменном сервере авторизации (`TcpListener`, как в `mcp_http.rs`):

- `resource` передаётся в обоих запросах;
- `code_verifier` сходится с challenge;
- после обновления сохраняется новый токен обновления;
- два параллельных запроса токена дают одно обновление;
- `refresh_token_reused` стирает сессию.

### C-3 — провайдер (`infra/llm_providers/chatgpt.rs`)

- `ProviderKind::ChatGpt`, ветка в `provider_for`, источник токена из
  `llm_session::resolve`;
- `LlmError::PlanLimit` и `SignInRequired`, с правкой `llm_retry`.

Тесты на записанных потоках SSE (из C-0):

- текст, рассуждения, вызов с аргументами по частям;
- `incomplete` → `truncated`;
- `failed` с `usage_limit_exceeded` → `PlanLimit`, без повтора;
- `401` → одно обновление и повтор;
- `System` уходит в `instructions`, а не элементом.

Мутационный прогон по `docs/11-mutation-testing.md`.

### C-4 — команды и UI

- Команды из решения 5, `generate_handler!`, обёртки в `src/lib/chatgpt.ts`, хук
  `useChatGptSession`.
- `ProviderSettings`: вид «ChatGPT». Вместо адреса и ключа — кнопка «Continue with
  ChatGPT»; после входа — карточка с email, «Connected», «Manage usage» и «Sign out».
  Модели — из `/v1/models`.
- Composer: «Using ChatGPT plan» возле выбора модели. Приветственное окно один раз —
  флаг в настройках. Окно `PlanLimit` с «Manage usage».
- Тесты на `bun test`: кнопка, карточка и окно лимита.

### C-5 — документы и проверка целиком

- `08-data-policy.md`, `docs/README.md`, строка в `CHANGELOG.md`.
- Прогон `agent_bench` на модели через ChatGPT. Нужен способ передать бенчу сессию вместо
  `AGENT_BENCH_API_KEY` — например, `AGENT_BENCH_CHATGPT_PROVIDER=<id>` берёт сессию из
  хранилища.
- `tsc`, `bun test`, `cargo test`.

## Вне плана

- **Responses API с обычным API-ключом.** Тот же `chatgpt.rs` со статическим токеном дал
  бы провайдера OpenAI на Responses API. Отдельное решение, когда понадобится.
- **WebSocket-режим Responses.** Продолжение по `previous_response_id` внутри одного
  соединения сэкономило бы пересылку истории. Сначала HTTP.
- **Сводка использования** (30 дней, пик, активные дни) из guidelines. Не видно, каким API
  её получать; пока только ссылка «Manage usage».
- **Подпись ID-токена по JWKS** — см. вопрос 3.

## Открытые вопросы

1. **Логотип ChatGPT на кнопке.** Ассеты бренда лежат в devkit, а у devkit лицензия
   Noncommercial. Можно ли взять сами знаки под brand guidelines OpenAI отдельно от кода?
   Если нельзя — текстовая кнопка «Continue with ChatGPT» без логотипа.
2. **Форма инструментов** (namespace / `additional_tools` против `tools`) — ответит C-0.
3. **Проверять ли подпись ID-токена.** OIDC разрешает обойтись проверкой TLS, но OpenAI
   пишет «verify against OpenAI's published JWKS». Рекомендация: без подписи в C-1;
   `jsonwebtoken` — только если C-0 покажет, что JWKS опубликован по discovery.
4. **Kubernetes-роль и `webSearch` на этом провайдере** — работают ли без изменений.
   Проверить в C-5 прогоном `kube_bench`.
