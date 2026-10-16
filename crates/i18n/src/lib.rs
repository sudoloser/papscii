//! Papscii localization.
//!
//! Every user-visible string is a keyed message in a Fluent (`.ftl`) resource,
//! Android-style: no literal UI text lives in the app code. Add locales by
//! dropping `<lang>.ftl` next to `en.ftl` and registering it here.

use fluent::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

const EN_FTL: &str = include_str!("../locales/en.ftl");

/// Holds loaded translation bundles and the active locale.
pub struct Localizer {
    bundle: FluentBundle<FluentResource>,
}

impl Localizer {
    /// Build a localizer for English (the fallback locale).
    pub fn english() -> Self {
        let langid: LanguageIdentifier = "en".parse().expect("valid langid");
        let mut bundle = FluentBundle::new(vec![langid]);
        let res = FluentResource::try_new(EN_FTL.to_string()).expect("en.ftl failed to parse");
        bundle.add_resource(res).expect("en.ftl failed to load");
        Localizer { bundle }
    }

    /// Look up `key` and return the string, or `key` itself if missing.
    pub fn text(&self, key: &str) -> String {
        self.format(key, None)
    }

    /// Look up `key` with Fluent arguments (`{ $name }` placeholders).
    ///
    /// Keys are authored Android-style with dots (`gallery.card.use`); Fluent
    /// message identifiers forbid dots, so they are stored hyphenated and
    /// normalized here.
    pub fn format(&self, key: &str, args: Option<&FluentArgs>) -> String {
        let id = key.replace('.', "-");
        let Some(msg) = self.bundle.get_message(&id) else {
            return key.to_string();
        };
        let Some(pattern) = msg.value() else {
            return key.to_string();
        };
        let mut errors = Vec::new();
        self.bundle
            .format_pattern(pattern, args, &mut errors)
            .into_owned()
    }
}

impl Default for Localizer {
    fn default() -> Self {
        Self::english()
    }
}

/// Build Fluent arguments from key/value pairs.
pub fn args<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> FluentArgs<'a> {
    let mut a = FluentArgs::new();
    for (k, v) in pairs {
        a.set(k, FluentValue::from(v));
    }
    a
}
