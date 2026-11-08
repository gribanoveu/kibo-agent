//! Keyboard layouts the optional wrong-layout correction can select after Fix.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyboardLayout {
    Ru,
    En,
}

impl KeyboardLayout {
    pub fn language_code(self) -> &'static str {
        match self {
            Self::Ru => "ru",
            Self::En => "en",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_names_are_language_codes() {
        assert_eq!(serde_json::from_str::<KeyboardLayout>("\"ru\"").unwrap(), KeyboardLayout::Ru);
        assert_eq!(serde_json::from_str::<KeyboardLayout>("\"en\"").unwrap(), KeyboardLayout::En);
        assert!(serde_json::from_str::<KeyboardLayout>("\"fr\"").is_err());
    }
}
