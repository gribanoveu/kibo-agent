//! The checklist's tool: its arguments, read leniently, and its tasks.

use super::*;

/// `todo` arguments. One wire tool, two operations — the same shape the model
/// is given, so nothing has to fan it out.
///
/// Read through [`LooseTodo`]: `op` decides when it says `write` or `update`,
/// and the fields sent decide when it does not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", try_from = "LooseTodo")]
pub enum TodoArgs {
    /// Appends to the end of the list. Never replaces it.
    Write { tasks: Vec<String> },
    Update {
        /// Which task to change. Omitted means the active one.
        ///
        /// The default exists because naming an id is a step that goes wrong
        /// silently: in the transcript that prompted it, the model closed `t6`
        /// meaning `t5`, and the checklist ended the turn claiming a finished
        /// step was still outstanding. "The task I am on" needs no id, and that
        /// is what almost every update means.
        #[serde(default)]
        id: Option<String>,
        /// Several tasks changed the same way in one call — duplicates
        /// cancelled, steps finished together. In the transcript that prompted
        /// it, closing seven duplicates one call each cost seven calls, after
        /// a first try named them `"t9..t15"` and was refused.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ids: Vec<String>,
        /// Absent, with a `note`: progress on the task, which stays as it
        /// is. The schema the model sees has always allowed leaving it out —
        /// and models did, with a note, to record what they had found.
        #[serde(default)]
        status: Option<TodoUpdateStatus>,
        #[serde(default)]
        note: Option<String>,
    },
}

/// A `todo` call as models send it. In `agent_bench`, deepseek-flash put the
/// task list in `op` as a JSON string (`"op": "[{…"`), put a task's title
/// there, or left `op` out — a round lost each time, for a call whose fields
/// said plainly what it was.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LooseTodo {
    #[serde(default)]
    op: Option<serde_json::Value>,
    #[serde(default)]
    tasks: Option<serde_json::Value>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    ids: Vec<String>,
    #[serde(default)]
    status: Option<TodoUpdateStatus>,
    #[serde(default)]
    note: Option<String>,
}

impl TryFrom<LooseTodo> for TodoArgs {
    type Error = String;

    fn try_from(call: LooseTodo) -> Result<Self, String> {
        let op = call.op.as_ref().and_then(serde_json::Value::as_str).map(|op| op.trim().to_ascii_lowercase());
        let update = || TodoArgs::Update { id: call.id.clone(), ids: call.ids.clone(), status: call.status, note: call.note.clone() };
        match op.as_deref() {
            Some("write") => {
                let tasks = call.tasks.as_ref().ok_or("op \"write\" needs tasks: a list of task titles")?;
                Ok(TodoArgs::Write { tasks: task_titles(tasks, true)? })
            }
            Some("update") => Ok(update()),
            // `op` missing or not an operation: what was sent says which.
            _ => {
                if let Some(tasks) = &call.tasks {
                    return Ok(TodoArgs::Write { tasks: task_titles(tasks, true)? });
                }
                if let Some(Ok(tasks)) = call.op.as_ref().map(|op| task_titles(op, false)) {
                    return Ok(TodoArgs::Write { tasks });
                }
                if call.id.is_some() || !call.ids.is_empty() || call.status.is_some() || call.note.is_some() {
                    return Ok(update());
                }
                Err(format!(
                    "op must be \"write\" (with tasks, a list of task titles) or \"update\" (with status or note); got {}",
                    call.op.map_or("nothing".to_string(), |op| op.to_string())
                ))
            }
        }
    }
}

/// Task titles from a list of strings, of `{"title": …}` objects, or of either
/// written out as a JSON string. A lone string is one title only where
/// `tasks` holds it — in `op` it is more likely a mistyped operation.
fn task_titles(value: &serde_json::Value, lone_string_is_a_title: bool) -> Result<Vec<String>, String> {
    use serde_json::Value;
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::String(title) => Ok(title.clone()),
                Value::Object(task) => ["title", "content", "task"]
                    .iter()
                    .find_map(|key| task.get(*key).and_then(Value::as_str))
                    .map(str::to_string)
                    .ok_or_else(|| format!("a task needs a title, got {item}")),
                other => Err(format!("a task title is a string, got {other}")),
            })
            .collect(),
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(list @ Value::Array(_)) => task_titles(&list, false),
            _ if lone_string_is_a_title => Ok(vec![text.clone()]),
            _ => Err(format!("expected a list of task titles, got {value}")),
        },
        other => Err(format!("tasks is a list of task titles, got {other}")),
    }
}

/// One entry in the model's checklist for a multi-step turn.
///
/// `id` is assigned by the runtime (`t1`, `t2`, …), never chosen by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub status: TodoStatus,
    /// A short result when completed, or the reason when cancelled.
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

/// What an update may set — deliberately not `Pending` or `InProgress`.
///
/// Excluding them at the type level is what makes "the model never picks the
/// next task itself" a fact the schema enforces rather than something the
/// prompt asks for. The runtime advances the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TodoUpdateStatus {
    Completed,
    Cancelled,
}

impl From<TodoUpdateStatus> for TodoStatus {
    fn from(status: TodoUpdateStatus) -> Self {
        match status {
            TodoUpdateStatus::Completed => TodoStatus::Completed,
            TodoUpdateStatus::Cancelled => TodoStatus::Cancelled,
        }
    }
}
