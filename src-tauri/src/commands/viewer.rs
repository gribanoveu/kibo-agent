//! Markdown files opened with the app — a double click in Finder or Explorer,
//! "Open With" — each in a small window of its own (`viewer.html`), not the
//! main one: reading a README should not open a folder, start its index and
//! its MCP servers.
//!
//! The window is given its file here, and the page asks for it by nothing but
//! being that window: a viewer reads the one file it was opened on, never a
//! path the page names.

use crate::sync::lock;
use serde::Serialize;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// The prefix of every viewer's label; the main window's is `main`.
pub const LABEL: &str = "viewer-";

#[derive(Default)]
struct Inner {
    /// Files that came before the app was set up (macOS sends them before
    /// `Ready`); `None` once it is, and a file opens as it comes.
    early: Option<Vec<PathBuf>>,
    /// Each open viewer's file, by its window's label.
    files: HashMap<String, PathBuf>,
    next: usize,
}

pub struct Viewers(Mutex<Inner>);

impl Default for Viewers {
    fn default() -> Self {
        Self(Mutex::new(Inner { early: Some(Vec::new()), ..Inner::default() }))
    }
}

impl Viewers {
    /// The files that came early, once: after this every file opens as it comes.
    pub fn take_early(&self) -> Vec<PathBuf> {
        lock(&self.0).early.take().unwrap_or_default()
    }

    /// Keeps `files` for later if the app is not set up yet; else hands them back to open now.
    fn hold(&self, files: Vec<PathBuf>) -> Vec<PathBuf> {
        match lock(&self.0).early.as_mut() {
            Some(early) => {
                early.extend(files);
                Vec::new()
            }
            None => files,
        }
    }

    /// A new viewer's label, with its file kept under it.
    fn add(&self, file: PathBuf) -> String {
        let mut inner = lock(&self.0);
        inner.next += 1;
        let label = format!("{LABEL}{}", inner.next);
        inner.files.insert(label.clone(), file);
        label
    }

    fn file(&self, label: &str) -> Option<PathBuf> {
        lock(&self.0).files.get(label).cloned()
    }
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

/// The Markdown files among a launch's arguments, the program itself left out;
/// a relative one is taken from `cwd`, where the launch was made.
pub fn markdown_args(args: impl IntoIterator<Item = OsString>, cwd: &Path) -> Vec<PathBuf> {
    args.into_iter()
        .skip(1)
        .map(|arg| cwd.join(arg))
        .filter(|path| is_markdown(path) && path.is_file())
        .collect()
}

/// The Markdown files among the URLs macOS hands over.
pub fn markdown_urls(urls: &[url::Url]) -> Vec<PathBuf> {
    urls.iter()
        .filter_map(|url| url.to_file_path().ok())
        .filter(|path| is_markdown(path) && path.is_file())
        .collect()
}

/// Opens a viewer on each of `files` — or keeps them, when the app is not set up yet.
pub fn open(app: &AppHandle, files: Vec<PathBuf>) {
    let viewers = app.state::<Arc<Viewers>>();
    for file in viewers.hold(files) {
        if let Err(e) = open_one(app, &viewers, file) {
            eprintln!("the file did not open: {e}");
        }
    }
}

/// The file's name, for the window's title.
fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A viewer is framed as the main window is — its config entry, for the
/// platform's: macOS's native traffic lights over the app's own bar, Windows'
/// undecorated window with the app's caption buttons — at a reading size.
fn open_one(app: &AppHandle, viewers: &Viewers, file: PathBuf) -> tauri::Result<()> {
    let mut config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
        .unwrap_or_default();
    config.title = name_of(&file);
    config.label = viewers.add(file);
    config.url = WebviewUrl::App("viewer.html".into());
    config.width = 820.0;
    config.height = 900.0;
    config.min_width = Some(360.0);
    config.min_height = Some(240.0);
    WebviewWindowBuilder::from_config(app, &config)?.build()?.set_focus()
}

#[derive(Debug, Serialize)]
pub struct ViewerFile {
    name: String,
    text: String,
}

/// The file this viewer was opened on: its name, for the title bar, and its text.
#[tauri::command]
pub async fn viewer_file(window: WebviewWindow, viewers: State<'_, Arc<Viewers>>) -> Result<ViewerFile, String> {
    let path = viewers.file(window.label()).ok_or("this window has no file")?;
    tauri::async_runtime::spawn_blocking(move || {
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(ViewerFile { name: name_of(&path), text })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kibo-viewer-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn markdown_args_keeps_existing_markdown_files_only() {
        let dir = temp_dir("args");
        for name in ["a.md", "B.MARKDOWN", "README.MD", "c.txt"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        std::fs::create_dir(dir.join("folder.md")).unwrap();
        let args = ["/app/kibo", "a.md", "B.MARKDOWN", "README.MD", "c.txt", "missing.md", "folder.md", "--flag"];
        let found = markdown_args(args.map(OsString::from), &dir);
        assert_eq!(found, vec![dir.join("a.md"), dir.join("B.MARKDOWN"), dir.join("README.MD")]);
    }

    #[test]
    fn markdown_args_leaves_out_the_program_itself() {
        let dir = temp_dir("program");
        std::fs::write(dir.join("kibo.md"), "x").unwrap();
        assert!(markdown_args([OsString::from("kibo.md")], &dir).is_empty());
    }

    #[test]
    fn markdown_args_keeps_an_absolute_path_as_it_is() {
        let dir = temp_dir("absolute");
        std::fs::write(dir.join("a.md"), "x").unwrap();
        let found = markdown_args(["kibo".into(), dir.join("a.md").into_os_string()], Path::new("/elsewhere"));
        assert_eq!(found, vec![dir.join("a.md")]);
    }

    #[test]
    fn markdown_urls_keeps_markdown_files_only() {
        let dir = temp_dir("urls");
        std::fs::write(dir.join("a.md"), "x").unwrap();
        std::fs::write(dir.join("b.txt"), "x").unwrap();
        let urls = [
            url::Url::from_file_path(dir.join("a.md")).unwrap(),
            url::Url::from_file_path(dir.join("b.txt")).unwrap(),
            url::Url::parse("https://example.com/c.md").unwrap(),
        ];
        assert_eq!(markdown_urls(&urls), vec![dir.join("a.md")]);
    }

    #[test]
    fn files_before_setup_wait_and_after_it_open_at_once() {
        let viewers = Viewers::default();
        assert!(viewers.hold(vec!["a.md".into()]).is_empty());
        assert!(viewers.hold(vec!["b.md".into()]).is_empty());
        assert_eq!(viewers.take_early(), vec![PathBuf::from("a.md"), PathBuf::from("b.md")]);
        assert_eq!(viewers.hold(vec!["c.md".into()]), vec![PathBuf::from("c.md")]);
        assert!(viewers.take_early().is_empty());
    }

    #[test]
    fn each_viewer_gets_its_own_label_and_reads_only_its_file() {
        let viewers = Viewers::default();
        let a = viewers.add("a.md".into());
        let b = viewers.add("b.md".into());
        assert_ne!(a, b);
        assert!(a.starts_with(LABEL) && b.starts_with(LABEL));
        assert_eq!(viewers.file(&a), Some("a.md".into()));
        assert_eq!(viewers.file(&b), Some("b.md".into()));
        assert_eq!(viewers.file("main"), None);
    }
}
