//! The kubeconfig files Chat mode's Kubernetes role is pointed at: kept in
//! `settings.json` by path, and looked for again as each reply is written.

use std::path::Path;

use crate::domain::chat_role::KubeSetup;
use crate::domain::settings::{KubeSettings, Kubeconfig, SettingsError};
use crate::infra::settings_store;

pub fn list() -> Result<KubeSettings, SettingsError> {
    Ok(settings_store::load()?.kube)
}

/// Adds `config`, or replaces the one of its name.
pub fn save(config: Kubeconfig) -> Result<(), SettingsError> {
    update(|kube| kube.upsert(config))
}

pub fn remove(name: &str) -> Result<(), SettingsError> {
    update(|kube| kube.remove(name))
}

/// The one the role works with; `None` goes back to the first.
pub fn pick(name: Option<String>) -> Result<(), SettingsError> {
    update(|kube| kube.active = name)
}

/// What the role is told: checked now rather than trusted from when it was
/// saved — a file moved since is the likeliest reason a command would fail.
pub fn setup() -> Result<KubeSetup, SettingsError> {
    Ok(match list()?.active() {
        None => KubeSetup::NotSet,
        Some(config) if Path::new(&config.path).is_file() => KubeSetup::Ready(config.clone()),
        Some(config) => KubeSetup::Missing(config.clone()),
    })
}

fn update(change: impl FnOnce(&mut KubeSettings)) -> Result<(), SettingsError> {
    let mut settings = settings_store::load()?;
    change(&mut settings.kube);
    settings_store::save(&settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    #[test]
    fn the_setup_follows_the_file_on_disk() {
        with_app_dir("kubeconfigs-setup", || {
            assert_eq!(setup().unwrap(), KubeSetup::NotSet);

            let file = crate::infra::app_dir::dir().unwrap().join("prod.yaml");
            let config = Kubeconfig { name: "prod".to_string(), path: file.to_string_lossy().into_owned() };
            save(config.clone()).unwrap();
            assert_eq!(setup().unwrap(), KubeSetup::Missing(config.clone()));

            std::fs::write(&file, "apiVersion: v1\n").unwrap();
            assert_eq!(setup().unwrap(), KubeSetup::Ready(config));

            remove("prod").unwrap();
            assert_eq!(setup().unwrap(), KubeSetup::NotSet);
        });
    }

    #[test]
    fn a_pick_is_kept_and_cleared() {
        with_app_dir("kubeconfigs-pick", || {
            for name in ["prod", "staging"] {
                save(Kubeconfig { name: name.to_string(), path: format!("/k/{name}") }).unwrap();
            }
            pick(Some("staging".to_string())).unwrap();
            assert_eq!(list().unwrap().active().map(|c| c.name.clone()).as_deref(), Some("staging"));
            pick(None).unwrap();
            assert_eq!(list().unwrap().active().map(|c| c.name.clone()).as_deref(), Some("prod"));
        });
    }
}
