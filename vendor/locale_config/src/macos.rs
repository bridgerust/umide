//! Inspect macOS system for locale configuration
use super::{LanguageRange, Locale};

use objc2_foundation::NSLocale;

pub fn system_locale() -> Option<Locale> {
    let current_locale = unsafe { NSLocale::currentLocale() };
    let locale_identifier = unsafe { current_locale.localeIdentifier() };
    Some(Locale::from(
        LanguageRange::from_unix(&locale_identifier.to_string()).ok()?,
    ))
}
