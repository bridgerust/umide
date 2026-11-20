//! Interface language for the native Workbench. Project and conversation data
//! retain their original text; only strings owned by the interface use this catalog.
use std::{collections::HashMap, sync::LazyLock};

use floem::{prelude::*, reactive::Context};
use serde::{Deserialize, Serialize};

use crate::{design, projects};

const CATALOGS: [&str; 5] = [
    include_str!("../locales/core.tsv"),
    include_str!("../locales/chat.tsv"),
    include_str!("../locales/files.tsv"),
    include_str!("../locales/tools.tsv"),
    include_str!("../locales/verification.tsv"),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Language {
    #[default]
    En,
    Fr,
    Es,
    Pt,
}

impl Language {
    const ALL: [Self; 4] = [Self::En, Self::Fr, Self::Es, Self::Pt];

    fn from_tag(tag: &str) -> Option<Self> {
        match tag
            .split(['-', '_', '.'])
            .next()?
            .to_ascii_lowercase()
            .as_str()
        {
            "en" => Some(Self::En),
            "fr" => Some(Self::Fr),
            "es" => Some(Self::Es),
            "pt" => Some(Self::Pt),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Fr => "Français",
            Self::Es => "Español",
            Self::Pt => "Português",
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::En => "EN",
            Self::Fr => "FR",
            Self::Es => "ES",
            Self::Pt => "PT",
        }
    }
}

#[derive(Clone, Copy)]
struct Locale {
    active: RwSignal<Language>,
}

pub(crate) fn install() {
    let choice = projects::data_dir().ok().and_then(|dir| {
        projects::load::<Language>(&dir.join("language.json"))
            .ok()
            .flatten()
    });
    let language = choice.unwrap_or_else(|| {
        sys_locale::get_locales()
            .find_map(|tag| Language::from_tag(&tag))
            .unwrap_or_default()
    });
    Context::provide(Locale {
        active: RwSignal::new(language),
    });
}

fn current() -> Language {
    Context::get::<Locale>()
        .map(|locale| locale.active.get())
        .unwrap_or_default()
}

pub(crate) fn language_name() -> &'static str {
    current().name()
}

struct Catalog {
    rows: HashMap<&'static str, [&'static str; 4]>,
    // Most-specific template first so a generic error cannot hide a specific one.
    patterns: Vec<&'static str>,
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    let mut rows = HashMap::new();
    for source in CATALOGS {
        for line in source
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let mut columns = line.split('\t');
            if let (Some(en), Some(fr), Some(es), Some(pt)) = (
                columns.next(),
                columns.next(),
                columns.next(),
                columns.next(),
            ) {
                rows.entry(en).or_insert([en, fr, es, pt]);
            }
        }
    }
    let mut patterns: Vec<_> = rows
        .keys()
        .copied()
        .filter(|key| key.contains('{'))
        .collect();
    patterns.sort_unstable_by(|a, b| {
        literal_len(b).cmp(&literal_len(a)).then_with(|| a.cmp(b))
    });
    Catalog { rows, patterns }
});

fn field(template: &str) -> Option<(&str, &str, &str)> {
    let (prefix, rest) = template.split_once('{')?;
    let (name, suffix) = rest.split_once('}')?;
    Some((prefix, name, suffix))
}

fn literal_len(mut template: &str) -> usize {
    let mut n = 0;
    while let Some((literal, _, rest)) = field(template) {
        n += literal.len();
        template = rest;
    }
    n + template.len()
}

fn translation(lang: Language, key: &'static str) -> &'static str {
    CATALOG.rows.get(key).map_or(key, |row| row[lang as usize])
}

pub(crate) fn tr(key: &'static str) -> &'static str {
    translation(current(), key)
}

pub(crate) fn label(key: &'static str) -> Label {
    Label::derived(move || tr(key))
}

fn render(mut template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    while let Some((literal, name, rest)) = field(template) {
        out.push_str(literal);
        if let Some((_, value)) = args.iter().find(|(key, _)| *key == name) {
            out.push_str(value);
        } else {
            out.push('{');
            out.push_str(name);
            out.push('}');
        }
        template = rest;
    }
    out.push_str(template);
    out
}

pub(crate) fn fmt(key: &'static str, args: &[(&str, &str)]) -> String {
    render(tr(key), args)
}

// Match an English message emitted by a worker without interpreting its field
// values. Paths, tool output and filenames can themselves contain braces.
fn captures<'a>(
    mut pattern: &'static str,
    mut text: &'a str,
) -> Option<Vec<(&'static str, &'a str)>> {
    let mut values = Vec::new();
    while let Some((prefix, name, rest)) = field(pattern) {
        text = text.strip_prefix(prefix)?;
        if let Some((delimiter, _, _)) = field(rest) {
            if delimiter.is_empty() {
                return None;
            }
            let end = text.find(delimiter)?;
            values.push((name, &text[..end]));
            text = &text[end..];
            pattern = rest;
        } else {
            values.push((name, text.strip_suffix(rest)?));
            return Some(values);
        }
    }
    None
}

pub(crate) fn message(text: &str) -> String {
    let lang = current();
    if lang != Language::En {
        if let Some(row) = CATALOG.rows.get(text) {
            return row[lang as usize].into();
        }
        for pattern in &CATALOG.patterns {
            if let Some(args) = captures(pattern, text) {
                return render(translation(lang, pattern), &args);
            }
        }
    }
    text.to_owned()
}

pub(crate) fn picker(compact: bool) -> impl IntoView {
    let locale = Context::get::<Locale>().expect("Workbench language is installed");
    let expanded = RwSignal::new(false);
    let problem = RwSignal::new(None::<String>);
    let trigger = Button::new(
        Stack::horizontal((
            design::icon(design::Glyph::Globe, 15., design::muted),
            label("Language").style(move |s| {
                s.font_size(12)
                    .flex_grow(1_f32)
                    .apply_if(compact, |s| s.hide())
            }),
            Label::derived(move || locale.active.get().name()).style(move |s| {
                s.font_size(11)
                    .color(design::muted())
                    .apply_if(compact, |s| s.hide())
            }),
            design::icon(design::Glyph::Chevron, 10., design::faint)
                .style(move |s| s.apply_if(compact, |s| s.hide())),
        ))
        .style(move |s| {
            s.width_full()
                .items_center()
                .gap(7)
                .apply_if(compact, |s| s.justify_center())
        }),
    );
    let trigger_id = trigger.id();
    let trigger = trigger
        .action(move || expanded.update(|open| *open = !*open))
        .style(|s| {
            s.width_full()
                .height(30)
                .padding_horiz(7)
                .border(0)
                .border_radius(5)
                .background(design::rail())
                .color(design::ink())
                .hover(|s| s.background(design::raised()))
                .focus_visible(|s| s.outline(2).outline_color(design::accent()))
        })
        .tooltip(|| tr("Language"));
    let options = Stack::vertical_from_iter(Language::ALL.map(move |lang| {
        Button::new(if compact { lang.code() } else { lang.name() })
            .action(move || {
                let result = projects::data_dir().and_then(|dir| {
                    projects::save(&dir.join("language.json"), &lang)
                });
                locale.active.set(lang);
                problem.set(result.err());
                expanded.set(false);
                trigger_id.request_focus();
            })
            .style(move |s| {
                let active = locale.active.get() == lang;
                s.width_full()
                    .height(26)
                    .font_size(12)
                    .border(0)
                    .padding_horiz(4)
                    .background(if active {
                        design::mint_wash()
                    } else {
                        design::panel()
                    })
                    .color(if active {
                        design::accent()
                    } else {
                        design::ink()
                    })
                    .border_radius(4)
                    .hover(|s| s.background(design::raised()))
                    .focus_visible(|s| s.outline(2).outline_color(design::accent()))
            })
            .tooltip(move || lang.name())
    }))
    .style(move |s| {
        s.width_full()
            .gap(2)
            .apply_if(!expanded.get(), |s| s.hide())
    });
    let error = Label::derived(move || {
        problem.with(|e| {
            e.as_ref()
                .map(|detail| {
                    fmt(
                        "Could not save the language preference: {detail}",
                        &[("detail", detail)],
                    )
                })
                .unwrap_or_default()
        })
    })
    .style(move |s| {
        s.width_full()
            .font_size(11)
            .color(design::danger())
            .text_overflow(floem::style::TextOverflow::Wrap)
            .apply_if(problem.with(Option::is_none), |s| s.hide())
    });
    Stack::vertical((trigger, options, error))
        .style(|s| s.width_full().gap(2))
        .on_key_down(
            Key::Named(NamedKey::Escape),
            |_| true,
            move |_| {
                expanded.set(false);
                trigger_id.request_focus();
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::reactive::Scope;

    fn names(mut template: &str) -> Vec<&str> {
        let mut fields = Vec::new();
        while let Some((_, name, rest)) = field(template) {
            assert!(
                !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "invalid placeholder in {template:?}"
            );
            fields.push(name);
            template = rest;
        }
        fields.sort_unstable();
        fields
    }

    #[test]
    fn catalogs_have_all_translations_and_preserve_arguments() {
        let mut keys = std::collections::HashSet::new();
        for catalog in CATALOGS {
            for line in catalog
                .lines()
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
            {
                let columns: Vec<_> = line.split('\t').collect();
                assert_eq!(columns.len(), 4, "{line}");
                assert!(
                    keys.insert(columns[0]),
                    "duplicate translation key: {}",
                    columns[0]
                );
                let expected = names(columns[0]);
                for value in columns {
                    assert!(!value.trim().is_empty(), "{line}");
                    assert_eq!(names(value), expected, "{line}");
                }
            }
        }
    }

    #[test]
    fn switching_language_updates_existing_readers_and_locale_tags_match_regions() {
        assert_eq!(Language::from_tag("fr-CA"), Some(Language::Fr));
        assert_eq!(Language::from_tag("es_MX.UTF-8"), Some(Language::Es));
        assert_eq!(Language::from_tag("pt-PT"), Some(Language::Pt));
        assert_eq!(Language::from_tag("ja-JP"), None);
        let scope = Scope::new();
        scope.enter(|| {
            let active = RwSignal::new(Language::En);
            Context::provide(Locale { active });
            let caption = floem::reactive::Memo::new(move |_| tr("Files"));
            assert_eq!(caption.get(), "Files");
            for (lang, name) in [
                (Language::Fr, "Fichiers"),
                (Language::Es, "Archivos"),
                (Language::Pt, "Arquivos"),
            ] {
                active.set(lang);
                assert_eq!(caption.get(), name);
            }
            assert_eq!(
                message("Could not open the task folder: repo/{status}.rs"),
                "Não foi possível abrir a pasta da tarefa: repo/{status}.rs"
            );
            assert_eq!(
                message("arbitrary command bytes"),
                "arbitrary command bytes"
            );
        });
        scope.dispose();
    }

    #[test]
    fn named_fields_can_be_reordered_without_changing_the_values() {
        let args = captures("From {old} to {new}.", "From {new} to src/nouveau.rs.")
            .unwrap();
        assert_eq!(render("{new} ← {old}", &args), "src/nouveau.rs ← {new}");
        assert!(captures("Closed ({status})", "Closed (unfinished").is_none());
    }
}
