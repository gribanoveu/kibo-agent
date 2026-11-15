//! The scenes the model painted, on disk (`docs/55-frost-scenes.md`, decision 8):
//! `~/.kibo/scenes/<id>.html`, each body as it came, and `scenes/index.json`
//! with their ids, titles and times.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::scene::{self, SceneError, SceneInfo};
use crate::infra::app_dir;

const DIR: &str = "scenes";
const INDEX: &str = "index.json";

/// From the read of the index to its write: two changes at once would lose one.
static LOCK: Mutex<()> = Mutex::new(());

fn store(e: impl std::fmt::Display) -> SceneError {
    SceneError::Store(e.to_string())
}

fn dir() -> Result<PathBuf, SceneError> {
    Ok(app_dir::dir().map_err(store)?.join(DIR))
}

fn body_path(id: &str) -> Result<PathBuf, SceneError> {
    Ok(dir()?.join(format!("{}.html", scene::id(id)?)))
}

/// The index as kept, oldest first; none is no scenes.
fn index() -> Result<Vec<SceneInfo>, SceneError> {
    match fs::read(dir()?.join(INDEX)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(store),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(store(e)),
    }
}

fn save_index(rows: &[SceneInfo]) -> Result<(), SceneError> {
    let bytes = serde_json::to_vec(rows).map_err(store)?;
    app_dir::write_private(&dir()?.join(INDEX), &bytes).map_err(store)
}

/// Every scene, newest first.
pub fn list() -> Result<Vec<SceneInfo>, SceneError> {
    let mut rows = index()?;
    rows.reverse();
    Ok(rows)
}

/// `body` kept under `title`, once the domain lets it in and there is room.
pub fn add(title: &str, body: &str) -> Result<SceneInfo, SceneError> {
    let title = scene::check(title, body)?;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut rows = index()?;
    scene::room(rows.len())?;
    let created = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let info = SceneInfo { id: uuid::Uuid::new_v4().to_string(), title: title.to_string(), created };
    // The body before its row: a row with no file would be a card with nothing on it.
    app_dir::write_private(&body_path(&info.id)?, body.as_bytes()).map_err(store)?;
    rows.push(info.clone());
    save_index(&rows)?;
    Ok(info)
}

/// Scene `id`'s body.
pub fn read(id: &str) -> Result<String, SceneError> {
    match fs::read_to_string(body_path(id)?) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(SceneError::NotFound(id.to_string())),
        read => read.map_err(store),
    }
}

/// Scene `id` gone, its row and its file; one already gone is no error.
pub fn delete(id: &str) -> Result<(), SceneError> {
    let path = body_path(id)?;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut rows = index()?;
    let before = rows.len();
    rows.retain(|row| row.id != id);
    // The row before its file: a file with no row is never seen, but a row
    // with no file is a scene the list leaves out and the limit still counts.
    if rows.len() != before {
        save_index(&rows)?;
    }
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(store(e)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    const BODY: &str = "<style>i{animation:a 9s infinite}</style><i></i>";

    #[test]
    fn a_scene_is_kept_and_read_back_and_listed_newest_first() {
        with_app_dir("scenes-add", || {
            assert_eq!(list(), Ok(Vec::new()), "no folder is no scenes");
            let first = add(" Forest ", BODY).unwrap();
            let second = add("Lake", "<i></i>").unwrap();
            assert_eq!(first.title, "Forest");
            assert_eq!(read(&first.id), Ok(BODY.to_string()));
            assert_eq!(read(&second.id), Ok("<i></i>".to_string()));
            assert_eq!(list().unwrap(), vec![second, first]);
        });
    }

    #[test]
    fn a_scene_the_domain_refuses_leaves_nothing_on_disk() {
        with_app_dir("scenes-refused", || {
            assert_eq!(add("Bad", "<script>x</script>"), Err(SceneError::Tag("script")));
            assert!(!dir().unwrap().exists());
        });
    }

    #[test]
    fn a_delete_takes_the_file_and_the_row() {
        with_app_dir("scenes-delete", || {
            let kept = add("Kept", BODY).unwrap();
            let gone = add("Gone", BODY).unwrap();
            delete(&gone.id).unwrap();
            assert!(!body_path(&gone.id).unwrap().exists());
            assert_eq!(list().unwrap(), vec![kept]);
            assert_eq!(read(&gone.id), Err(SceneError::NotFound(gone.id.clone())));
            assert_eq!(delete(&gone.id), Ok(()), "twice is no error");
        });
    }

    #[test]
    fn an_id_that_is_not_a_uuid_is_refused_before_the_disk() {
        with_app_dir("scenes-bad-id", || {
            let outside = dir().unwrap().parent().unwrap().join("x.html");
            fs::write(&outside, "keep me").unwrap();
            assert_eq!(read("../x"), Err(SceneError::BadId("../x".into())));
            assert_eq!(delete("../x"), Err(SceneError::BadId("../x".into())));
            assert!(outside.exists());
        });
    }

    #[test]
    fn the_fifty_first_scene_is_refused() {
        with_app_dir("scenes-full", || {
            for i in 0..scene::MAX_SCENES {
                add(&format!("Scene {i}"), "<i></i>").unwrap();
            }
            assert_eq!(add("One more", "<i></i>"), Err(SceneError::Full));
            assert_eq!(list().unwrap().len(), scene::MAX_SCENES);
        });
    }
}
