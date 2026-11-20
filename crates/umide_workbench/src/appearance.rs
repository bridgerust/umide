//! Workbench appearance follows the operating system unless the user pins a theme.
use std::time::Duration;

use floem::{action, menu::Menu, prelude::*, reactive::Context, window::Theme};
use serde::{Deserialize, Serialize};

use crate::{design::*, projects, state::Workbench};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preference {
    #[default]
    System,
    Light,
    Dark,
}

impl Preference {
    pub fn load() -> Self {
        projects::data_dir()
            .ok()
            .and_then(|dir| {
                projects::load(&dir.join("appearance.json")).ok().flatten()
            })
            .unwrap_or_default()
    }

    pub fn native(self) -> Option<Theme> {
        match self {
            Self::System => None,
            Self::Light => Some(Theme::Light),
            Self::Dark => Some(Theme::Dark),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Appearance {
    preference: RwSignal<Preference>,
    light: RwSignal<bool>,
}

impl Appearance {
    pub fn install(preference: Preference) -> Self {
        let appearance = Self {
            preference: RwSignal::new(preference),
            light: RwSignal::new(
                preference.native().unwrap_or(Theme::Light) == Theme::Light,
            ),
        };
        Context::provide(appearance);
        if let Some(theme) = preference.native() {
            action::set_theme(Some(theme));
        }
        appearance
    }

    pub fn window_changed(self, theme: Theme) {
        let effective = self.preference.get_untracked().native().unwrap_or(theme);
        let light = effective == Theme::Light;
        if self.light.get_untracked() != light {
            self.light.set(light);
        }
    }

    fn choose(self, preference: Preference, wb: Workbench) {
        if self.preference.get_untracked() == preference {
            return;
        }
        self.preference.set(preference);
        action::set_theme(preference.native());
        if let Some(theme) = preference.native() {
            self.window_changed(theme);
        } else {
            // The window's effective appearance is current once Floem has removed the override.
            action::exec_after(Duration::ZERO, move |_| {
                if self.preference.try_get_untracked() == Some(Preference::System) {
                    let theme = action::current_theme().unwrap_or(Theme::Light);
                    if self.light.get_untracked() != (theme == Theme::Light) {
                        self.window_changed(theme);
                        // Update Floem's built-in controls and leave the window following the OS.
                        action::set_theme(Some(theme));
                        action::set_theme(None);
                    }
                }
            });
        }
        if let Err(error) = projects::data_dir().and_then(|dir| {
            projects::save(&dir.join("appearance.json"), &preference)
        }) {
            wb.status.set(error);
        }
    }

    fn menu(self, wb: Workbench) -> Menu {
        let selected = self.preference.get_untracked();
        [Preference::System, Preference::Light, Preference::Dark]
            .into_iter()
            .fold(Menu::new(), |menu, choice| {
                menu.item(choice.name(), |item| {
                    item.checked(selected == choice)
                        .action(move || self.choose(choice, wb))
                })
            })
    }
}

pub fn is_light() -> bool {
    Context::get::<Appearance>().is_some_and(|appearance| appearance.light.get())
}

pub fn picker(wb: Workbench, compact: bool) -> impl IntoView {
    let appearance =
        Context::get::<Appearance>().expect("window appearance is installed");
    let symbol = svg(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"><circle cx="12" cy="12" r="8.5"/><path fill="currentColor" stroke="none" d="M12 3.5a8.5 8.5 0 0 1 0 17z"/></svg>"#)
        .style(|s| s.size(16, 16).color(muted()));
    let label = Stack::horizontal((
        "Appearance".style(|s| s.color(ink()).font_size(12).flex_grow(1_f32)),
        Label::derived(move || appearance.preference.get().name())
            .style(|s| s.color(muted()).font_size(11)),
        icon(Glyph::Chevron, 10., faint),
    ))
    .style(move |s| {
        s.flex_grow(1_f32)
            .items_center()
            .gap(4)
            .apply_if(compact, |s| s.hide())
    });
    let control = Button::new(Stack::horizontal((symbol, label)).style(move |s| {
        s.width_full()
            .items_center()
            .gap(8)
            .apply_if(compact, |s| s.justify_center())
    }));
    let id = control.id();
    control
        .action(move || {
            let bounds = id.get_layout_rect();
            action::show_context_menu(
                appearance.menu(wb),
                Some((bounds.x0, bounds.y1).into()),
            );
        })
        .style(move |s| {
            s.width_full()
                .height(30)
                .padding_horiz(if compact { 0 } else { 7 })
                .border(0)
                .border_radius(6)
                .background(rail())
                .hover(|s| s.background(surface()))
                .focus_visible(|s| s.outline(2).outline_color(accent()))
        })
        .tooltip(move || {
            format!("Appearance · {}", appearance.preference.get().name())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_events_only_affect_the_automatic_preference() {
        let scope = floem::reactive::Scope::new();
        scope.enter(|| {
            let appearance = Appearance::install(Preference::System);
            appearance.window_changed(Theme::Dark);
            assert!(!is_light());
            appearance.window_changed(Theme::Light);
            assert!(is_light());
            appearance.preference.set(Preference::Dark);
            appearance.window_changed(Theme::Light);
            assert!(!is_light());
        });
        scope.dispose();
    }

    #[test]
    fn preference_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("appearance.json");
        for choice in [Preference::System, Preference::Dark, Preference::Light] {
            projects::save(&path, &choice).unwrap();
            assert_eq!(projects::load::<Preference>(&path).unwrap(), Some(choice));
        }
    }
}
