# App.tsx: композиционный корень худеет без Context

План, предложен 2026-11-21. Рефакторинг `src/App.tsx` (1215 строк): связные кластеры
состояния и правил переезжают в хуки `src/hooks/` и render-only компоненты, App остаётся
композиционным корнем на ~600 строк. Пользовательского изменения нет — поведение
сохраняется дословно, включая зависимости эффектов. Context, глобальный стор и новые
зависимости не вводятся.

Кода в документе нет — только решения.

## Что уже есть

- `src/App.tsx` — композиционный корень: ~90 импортов, ~30 вызовов хуков, 27
  `useState`/`useStoredState`, 7 `useEffect`, ~450 строк JSX. Кластеры (строки на момент
  плана): страницы сайдбара 126–150; раскладка 151–156, 218–250, 320–384; диалоги
  183–197 и 1070–1213; проводка сайдбара ~830–930; маршрутизация чатов 388–435, 530–598;
  команды композера 440–470.
- `src/lib/docks.ts` — правило «какая панель куда» уже вынесено (`openPane`, `togglePane`,
  `toggleTerminal`, `ideDocks`); в App осталось хранить ответ и решать про место окна.
- `src/components/Modal.tsx`, `Settings.tsx`, `McpInstall.tsx`, `ConfigEntryForm.tsx`,
  `ConfigFileEditor.tsx` — содержимое диалогов уже компоненты; в App живут обёртки `Modal`,
  состояние `mcpDialog`/`hooksDialog` и обработчики панелей.
- Идиома хуков: `useStoredState` (состояние вида в localStorage), `usePanelSizes`,
  `useShortcuts`, `useLiveRead`; тесты — `renderHook` в `src/__tests__/`
  (`layout.test.tsx`, `branchPicker.test.tsx` — образцы).
- **Ни один тест не рендерит `App` целиком** (проверено поиском по `src/__tests__/`):
  проводка держится только тестами частей. Это главный риск рефакторинга.

## Решения

### 1. Хуки с параметрами, не Context и не стор

AGENTS.md прямо держит правило: состояние живёт в хуках, Context не используется.
Кластеры читаются двумя-тремя поддеревьями, и все они уже получают данные пропсами из
App — хук с параметрами сохраняет тот же поток данных, только объявление переезжает.
Отвергнуто: Context (запрещён правилами проекта, прячет ре-рендеры), внешний стор
(новая зависимость и второй источник правды при живом первом).

### 2. Извлечение по кластерам, каждый этап — behavior-preserving

Кластер — связное состояние со своими правилами (раскладка, страницы, диалоги).
Отвергнуто: один `useAppState` на всё — переносит проблему целиком, не уменьшая связность.
Зависимости эффектов (`[onPage]`, `[pages]`, `[terminalPaste]`, `[workspace.error]`,
`[agent.turn.status, plain.turn.status, llm.reload]`) переносятся дословно; порядок
вызовов хуков диктуется данными и сохраняется: `shown` до `useAgentTurn`/`useMcp`,
флаги editing до `useMcp`/`useHooks`, `viewerOpen` до раскладки.

### 3. Цикл «раскладка ↔ viewer» разрывается двумя параметрами

`dockFits` зависит от `viewerOpen`, а `setDocks`/`toggleSidebar` зовут `viewer.closeAll`.
Хук раскладки берёт `{ viewerOpen, closeViewer }`, не объект viewer целиком. Отвергнуто:
передавать `useOpenFiles`-результат — тянет в раскладку файловые зависимости, которых
правило места не знает.

### 4. Каждый извлечённый хук получает свои renderHook-тесты

Интеграционного теста App нет, поэтому страховка извлечения — тесты на правила кластера,
включая ранее не покрытые: IDE-свап возвращает панели как просил пользователь; `setDocks`
без места закрывает viewer, кроме `plan`; удаление чата сначала `agent.discard`, потом
`history.remove`. Отвергнуто: интеграционный тест всего App — три десятка хуков и моков
бэкенда дороже, чем тесты правил по отдельности.

### 5. Диалоги — render-only компоненты, состояние — в хуке

`McpConfigDialog`, `HooksConfigDialog`, `SettingsDialog` только рисуют; состоянием
`mcpDialog`/`hooksDialog` и обработчиками панелей владеет `useConfigDialogs`. Это правило
слоёв AGENTS.md (components — render only), а не новая форма.

### 6. Мелкие сигналы между поддеревьями остаются в App

`quote`, `composerFocus`, `terminalPaste`, `processFocus`, `agentShown`, `headSlot` —
по одной-три строки каждый; хук для них стоит дороже, чем экономит. Пересмотреть, если
сигналов станет заметно больше.

### 7. Маршрутизация чатов — фаза 2, по решению после основных этапов

`selectChat`/`newChat`/`send`/`pickOffer`/команды — самый связный кластер
(agent + plain + workspace + history + folderSwitch + conversation + llm) с форвардными
замыканиями (`selectChat` ссылается на `folderSwitch`, объявленный ниже; `builtIn` — на
`send`). Выносится только если AD-1…AD-5 приземлились чисто и App всё ещё велик.

## Слои

Только фронтенд; бэкенд, команды и события не тронуты.

- `src/hooks/usePages.ts`, `useDocks.ts`, `useConfigDialogs.ts`, `useSidebarActions.ts` —
  состояние и правила кластеров.
- `src/components/McpConfigDialog.tsx`, `HooksConfigDialog.tsx`, `SettingsDialog.tsx` —
  render-only обёртки существующих компонентов; своих стилей не требуют (рисуют `Modal`).
- `src/__tests__/` — тесты хуков и порядка обработчиков.

## Нарезка

Каждый этап — отдельный коммит с тестами и мутационной проверкой
([11-mutation-testing.md](11-mutation-testing.md)); проверки этапа:
`bun run tsc --noEmit && bun test` (бэкенд не тронут — `cargo` не нужен).

**AD-1. usePages.** `page`, `pageItem`, `kept`, `keptPages`, `onPage` и два эффекта
(keep-страница монтируется навсегда; выключенная фича убирает страницу). Вход: `ide`,
`pages`. Тесты: kept-страница остаётся смонтированной после ухода; фича выключена —
страница выпадает из kept; в IDE `onPage` всегда null; `openPage` несёт item, уход —
стирает.

**AD-2. useDocks.** Раскладка: `tab`, `bottomTab`, `asideHidden`, `collapsed`,
`ideLayout`, media-запросы (`dockFits`, `sidebarFits`, `rail`), производные
(`topHidden`, `bottomShown`, `planSheet`, `dockShown`, `shown`), `docks`/`setDocks`,
`beforeIde`/`toggleIde`, `openTab`, эффект `terminalPaste`, `toggleSidebar`, панельные
клавиши. Вход: `{ agentMode, pageShown, viewerOpen, closeViewer }`. Тесты: IDE-свап
сохраняет и возвращает `beforeIde`; `setDocks` без места закрывает viewer, кроме plan;
`openTab`/`togglePane` по правилу `lib/docks.ts`; узкое окно — rail; `planSheet` только
в Agent без страницы.

**AD-3. Диалоги конфигов.** `useConfigDialogs(mcp, hooks)` — состояние и обработчики
панелей; `McpConfigDialog`, `HooksConfigDialog` — JSX. Тесты: удаление сервера/хука
пишет через `save`; переход «json» ↔ форма; ошибка editing не показывается в панели.

**AD-4. SettingsDialog.** Перенос `Modal` + `Settings` (~55 строк JSX) в компонент;
состояние `settingsOpen`/`settingsAt` остаётся в App (зовут из трёх поддеревьев);
звук при включении алертов переезжает вместе с JSX. Тест: включение звука проигрывает
его один раз, выключение — нет.

**AD-5. useSidebarActions.** Обработчики сайдбара (archive/pin/move/delete чатов,
группы) с ветвлением agent/plain. Тесты: удаление в Agent — сначала `discard`, потом
`remove`, при отказе `discard` строка не удаляется; удаление в Chat — `plain.remove` и
сброс черновика; ошибки групп возвращаются как `{ error }`, не бросаются.

**AD-6. Документы.** Итоговый статус плана и `docs/README.md`. Строки в CHANGELOG нет:
рефакторинг невидим пользователю.

## Вне плана

- `useChatRouting`/`useComposerCommands` (фаза 2) — вернуться, если AD-1…AD-5 сделаны
  чисто и App остаётся больше ~700 строк; решение за пользователем.
- Интеграционный тест App целиком — см. решение 4.
- `ChatPanel.tsx` (1512 строк) и другие крупные файлы — отдельные задачи.

## Открытые вопросы

1. Фаза 2 (`useChatRouting` + `useComposerCommands`) — делать ли после AD-5? Зависит от
   того, сколько строк останется в App и насколько чисто приземлятся основные этапы.