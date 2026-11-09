//! UI language: English and Portuguese.
//!
//! The default follows the Windows display language (Portuguese if Windows is in Portuguese,
//! English for everything else). The user can override it in the settings menu.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Pt,
}

/// What the user picked in settings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LangChoice {
    Auto,
    Fixed(Lang),
}

impl LangChoice {
    pub fn key(self) -> &'static str {
        match self {
            LangChoice::Auto => "auto",
            LangChoice::Fixed(Lang::En) => "en",
            LangChoice::Fixed(Lang::Pt) => "pt",
        }
    }
    pub fn from_key(s: &str) -> LangChoice {
        match s {
            "en" => LangChoice::Fixed(Lang::En),
            "pt" => LangChoice::Fixed(Lang::Pt),
            _ => LangChoice::Auto,
        }
    }
    pub fn resolve(self) -> Lang {
        match self {
            LangChoice::Auto => system_lang(),
            LangChoice::Fixed(l) => l,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == Lang::Pt as u8 { Lang::Pt } else { Lang::En }
}

/// Windows display language → Portuguese if it is any Portuguese variant, otherwise English.
pub fn system_lang() -> Lang {
    const LANG_PORTUGUESE: u16 = 0x16;
    let langid = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    if langid & 0x3FF == LANG_PORTUGUESE { Lang::Pt } else { Lang::En }
}

/// Picks the string for the current language.
pub fn tr(en: &'static str, pt: &'static str) -> &'static str {
    match current() {
        Lang::En => en,
        Lang::Pt => pt,
    }
}

/// Picks between two already-formatted strings.
pub fn trf(en: String, pt: String) -> String {
    match current() {
        Lang::En => en,
        Lang::Pt => pt,
    }
}
