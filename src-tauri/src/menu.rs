//! The macOS menu bar, built from the same catalog the window reads
//! (`docs/65-interface-language.md`, LC-3). `Menu::default` cannot be
//! translated — its labels are muda's own — so the menu is assembled here
//! item by item: the same submenus and predefined items, with the catalog's
//! `menu.*` texts. Rebuilt whenever the language is saved; Quit stays the
//! window's own, so the files typed into are saved before the app goes.

use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID, WINDOW_SUBMENU_ID};
use tauri::{AppHandle, Runtime};

use crate::domain::settings::UiLanguage;

/// The menu bar's Quit, given to the window to decide (`lib.rs` answers it).
pub const QUIT: &str = "quit";

const EN: &str = include_str!("../../src/locales/en.json");
const RU: &str = include_str!("../../src/locales/ru.json");

/// Every `menu.*` key the menu reads — a test holds both catalogs complete.
const MENU_KEYS: &[&str] = &[
    "menu.file",
    "menu.edit",
    "menu.view",
    "menu.window",
    "menu.help",
    "menu.about",
    "menu.services",
    "menu.hide",
    "menu.hide_others",
    "menu.quit",
    "menu.close_window",
    "menu.undo",
    "menu.redo",
    "menu.cut",
    "menu.copy",
    "menu.paste",
    "menu.select_all",
    "menu.fullscreen",
    "menu.minimize",
    "menu.zoom",
];

/// The menu in the language the setting picks, `auto` following the system.
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let language = crate::infra::settings_store::load().unwrap_or_default().ui_language;
    build_in(app, language.resolve(sys_locale::get_locale().as_deref()))
}

/// Every menu text in `language`, the app's name substituted; a key the
/// language lacks reads in English (decision 3). Pure: the platform items
/// below need macOS's main thread, which a test thread is not — the texts
/// are what a test can hold, the assembly is checked by hand.
fn texts(language: UiLanguage, name: &str) -> Vec<(&'static str, String)> {
    // The bundled catalogs are valid JSON — a test parses both.
    let catalog: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(if language == UiLanguage::Russian { RU } else { EN }).expect("the bundled catalog is valid JSON");
    let fallback: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(EN).expect("the bundled catalog is valid JSON");
    MENU_KEYS
        .iter()
        .map(|key| {
            let text = catalog
                .get(*key)
                .and_then(|v| v.as_str())
                .or_else(|| fallback.get(*key).and_then(|v| v.as_str()))
                .unwrap_or(key)
                .replace("{name}", name);
            (*key, text)
        })
        .collect()
}

fn build_in<R: Runtime>(app: &AppHandle<R>, language: UiLanguage) -> tauri::Result<Menu<R>> {
    let name = app.package_info().name.clone();
    let texts = texts(language, &name);
    let t = |key: &str| -> String {
        texts.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()).unwrap_or_else(|| key.to_string())
    };
    let about = AboutMetadata {
        name: Some(name.clone()),
        version: Some(app.package_info().version.to_string()),
        copyright: app.config().bundle.copyright.clone(),
        authors: app.config().bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };
    Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                name.clone(),
                true,
                &[
                    &PredefinedMenuItem::about(app, Some(&t("menu.about")), Some(about))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, Some(&t("menu.services")))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, Some(&t("menu.hide")))?,
                    &PredefinedMenuItem::hide_others(app, Some(&t("menu.hide_others")))?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, QUIT, t("menu.quit"), true, Some("CmdOrCtrl+Q"))?,
                ],
            )?,
            &Submenu::with_items(app, t("menu.file"), true, &[&PredefinedMenuItem::close_window(app, Some(&t("menu.close_window")))?])?,
            &Submenu::with_items(
                app,
                t("menu.edit"),
                true,
                &[
                    &PredefinedMenuItem::undo(app, Some(&t("menu.undo")))?,
                    &PredefinedMenuItem::redo(app, Some(&t("menu.redo")))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::cut(app, Some(&t("menu.cut")))?,
                    &PredefinedMenuItem::copy(app, Some(&t("menu.copy")))?,
                    &PredefinedMenuItem::paste(app, Some(&t("menu.paste")))?,
                    &PredefinedMenuItem::select_all(app, Some(&t("menu.select_all")))?,
                ],
            )?,
            &Submenu::with_items(app, t("menu.view"), true, &[&PredefinedMenuItem::fullscreen(app, Some(&t("menu.fullscreen")))?])?,
            &Submenu::with_id_and_items(
                app,
                WINDOW_SUBMENU_ID,
                t("menu.window"),
                true,
                &[
                    &PredefinedMenuItem::minimize(app, Some(&t("menu.minimize")))?,
                    &PredefinedMenuItem::maximize(app, Some(&t("menu.zoom")))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::close_window(app, Some(&t("menu.close_window")))?,
                ],
            )?,
            &Submenu::with_id_and_items(app, HELP_SUBMENU_ID, t("menu.help"), true, &[])?,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(text: &str) -> serde_json::Map<String, serde_json::Value> {
        serde_json::from_str(text).unwrap()
    }

    /// Both catalogs carry every menu key, and nothing else under `menu.*`.
    #[test]
    fn both_catalogs_have_every_menu_key() {
        for (label, text) in [("en", EN), ("ru", RU)] {
            let catalog = catalog(text);
            for key in MENU_KEYS {
                assert!(catalog.get(*key).and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()), "{label}: {key} is missing");
            }
            let extra: Vec<&String> = catalog.keys().filter(|k| k.starts_with("menu.") && !MENU_KEYS.contains(&k.as_str())).collect();
            assert!(extra.is_empty(), "{label}: menu keys the menu does not read: {extra:?}");
        }
    }

    /// The English texts read as muda's defaults did; the Russian ones are
    /// the catalog's, the app's name substituted.
    #[test]
    fn the_menu_texts_speak_the_language_they_were_built_in() {
        let get = |language, key: &str| -> String {
            texts(language, "Kibo Agent").into_iter().find(|(k, _)| *k == key).unwrap().1
        };
        assert_eq!(get(UiLanguage::English, "menu.file"), "File");
        assert_eq!(get(UiLanguage::English, "menu.quit"), "Quit Kibo Agent");
        assert_eq!(get(UiLanguage::English, "menu.zoom"), "Zoom");
        assert_eq!(get(UiLanguage::English, "menu.fullscreen"), "Toggle Full Screen");
        assert_eq!(get(UiLanguage::Russian, "menu.file"), "Файл");
        assert_eq!(get(UiLanguage::Russian, "menu.quit"), "Завершить Kibo Agent");
        assert_eq!(get(UiLanguage::Russian, "menu.hide"), "Скрыть Kibo Agent");
        assert_eq!(get(UiLanguage::Russian, "menu.undo"), "Отменить");
    }
}
