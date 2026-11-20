use floem::prelude::*;

use crate::{
    choose_project,
    design::*,
    i18n::{self, tr},
    state::Workbench,
};

fn project_picker(wb: Workbench, compact: bool) -> impl IntoView {
    let name = Label::derived(move || {
        wb.root.with(|root| {
            root.as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| tr("Open project…").to_owned())
        })
    })
    .style(|s| {
        s.color(ink())
            .font_size(13)
            .ui_bold()
            .width_full()
            .text_ellipsis()
    });
    let title = Stack::vertical((
        i18n::label("PROJECT").style(|s| s.color(faint()).font_size(10).ui_bold()),
        name,
    ))
    .style(move |s| {
        s.flex_grow(1_f32)
            .min_width(0)
            .gap(2)
            .apply_if(compact, |s| s.hide())
    });
    let content = Stack::horizontal((
        icon(Glyph::Folder, 17., accent),
        title,
        icon(Glyph::Chevron, 12., faint)
            .style(move |s| s.apply_if(compact, |s| s.hide())),
    ))
    .style(move |s| {
        s.width_full()
            .items_center()
            .gap(9)
            .apply_if(compact, |s| s.justify_center())
    });
    Button::new(content)
        .action(move || choose_project(wb))
        .style(move |s| {
            s.width_full()
                .height(if compact { 35 } else { 53 })
                .padding_horiz(if compact { 0 } else { 10 })
                .border(1)
                .border_color(line())
                .border_radius(8)
                .background(panel())
                .hover(|s| s.background(surface()))
                .focus_visible(|s| s.outline(2).outline_color(accent()))
                .set_disabled(wb.any_busy() || wb.launching.get())
        })
        .tooltip(move || {
            wb.root.with_untracked(|r| {
                r.as_ref()
                    .map(|p| {
                        i18n::fmt(
                            "Switch project · {path}",
                            &[("path", &p.display().to_string())],
                        )
                    })
                    .unwrap_or_else(|| tr("Open a project folder").into())
            })
        })
}

fn task_row(wb: Workbench, index: usize, compact: bool) -> impl IntoView {
    let is_claude = wb.history.with_untracked(|h| {
        h.conversations
            .get(index)
            .is_some_and(|c| c.provider == "claude")
    });
    let glyph = if is_claude {
        Glyph::Claude
    } else {
        Glyph::Codex
    };
    let provider = if is_claude { "Claude Code" } else { "Codex" };
    let color = move || if is_claude { peach() } else { lilac() };
    let symbol = icon(glyph, 15., color).container().style(|s| {
        s.size(26, 26)
            .border_radius(6)
            .items_center()
            .justify_center()
            .background(surface())
            .flex_shrink(0_f32)
    });
    let title = Label::derived(move || {
        wb.history.with(|h| {
            h.conversations
                .get(index)
                .map(|c| c.title.clone())
                .unwrap_or_default()
        })
    })
    .style(move |s| {
        s.color(if wb.current.get() == Some(index) {
            ink()
        } else {
            muted()
        })
        .width_full()
        .text_ellipsis()
        .font_size(12)
    });
    let sub = Label::derived(move || {
        let badge = wb.task_badge(index);
        if badge.is_empty() {
            provider.to_string()
        } else {
            format!("{provider} · {}", tr(badge))
        }
    })
    .style(move |s| {
        s.width_full().font_size(10).text_ellipsis().color(
            match wb.task_badge(index) {
                "Needs approval" => peach(),
                "Running" => accent(),
                _ => faint(),
            },
        )
    });
    let body = Stack::vertical((title, sub)).style(move |s| {
        s.flex_grow(1_f32)
            .min_width(0)
            .gap(2)
            .apply_if(compact, |s| s.hide())
    });
    let row = Stack::horizontal((symbol, body)).style(move |s| {
        s.width_full()
            .min_width(0)
            .items_center()
            .gap(8)
            .apply_if(compact, |s| s.justify_center())
    });
    Button::new(row)
        .action(move || wb.select(index))
        .style(move |s| {
            let active = wb.current.get() == Some(index);
            s.width_full()
                .min_width(0)
                .height(if compact { 38 } else { 47 })
                .padding_horiz(if compact { 0 } else { 7 })
                .padding_vert(0)
                .border_radius(7)
                .border(1)
                .border_color(if active { line() } else { rail() })
                .background(if active { surface() } else { rail() })
                .hover(|s| s.background(panel()))
                .focus_visible(|s| s.outline(2).outline_color(accent()))
        })
        .tooltip(move || {
            wb.history.with_untracked(|h| {
                h.conversations
                    .get(index)
                    .map(|c| format!("{provider} · {}", c.title))
                    .unwrap_or_default()
            })
        })
}

pub fn sidebar(
    wb: Workbench,
    compact: bool,
    toggle: impl Fn() + 'static,
    open_setup: impl Fn() + 'static,
) -> impl IntoView {
    let filter = RwSignal::new(String::new());
    let searching = RwSignal::new(false);
    let brand = Stack::horizontal((
        icon(Glyph::Brand, 24., on_accent).container().style(|s| {
            s.size(32, 32)
                .background(accent())
                .border_radius(9)
                .items_center()
                .justify_center()
        }),
        "Umide".style(move |s| {
            s.font_size(24)
                .ui_bold()
                .color(ink())
                .apply_if(compact, |s| s.hide())
        }),
    ))
    .style(move |s| {
        s.width_full()
            .height(57)
            .gap(10)
            .items_center()
            .padding_horiz(if compact { 0 } else { 6 })
            .apply_if(compact, |s| s.justify_center())
    });
    let new_button = Button::new(
        Stack::horizontal((
            icon(Glyph::Plus, 14., move || {
                if wb.root.with(Option::is_none) {
                    faint()
                } else {
                    accent()
                }
            }),
            i18n::label("New conversation").style(move |s| {
                s.ui_bold().font_size(12).apply_if(compact, |s| s.hide())
            }),
        ))
        .style(|s| s.items_center().gap(8)),
    )
    .action(move || wb.new_chat())
    .style(move |s| {
        s.width_full()
            .height(32)
            .padding(0)
            .border(1)
            .border_radius(7)
            .border_color(line())
            .background(mint_wash())
            .color(accent())
            .hover(|s| s.background(raised()))
            .focus_visible(|s| s.outline(2).outline_color(accent()))
            .set_disabled(wb.root.with(Option::is_none))
            .disabled(|s| {
                s.color(faint())
                    .background(panel())
                    .border_color(subtle_line())
            })
    })
    .tooltip(|| i18n::label("New conversation"));
    let field = text_input(filter).style(move |s| {
        s.width_full()
            .height(30)
            .color(ink())
            .padding_horiz(8)
            .background(panel())
            .border(1)
            .border_color(line())
            .border_radius(5)
            .font_size(12)
            .focus(|s| s.border_color(accent()))
    });
    let field_id = field.id();
    let search = Button::new(icon(Glyph::Search, 14., muted))
        .action(move || {
            searching.update(|v| *v = !*v);
            if searching.get_untracked() {
                field_id.request_focus();
            } else {
                filter.set(String::new());
            }
        })
        .style(|s| {
            s.size(24, 24)
                .padding(0)
                .items_center()
                .justify_center()
                .border(0)
                .background(rail())
                .border_radius(4)
                .hover(|s| s.background(raised()))
                .focus_visible(|s| s.outline(2).outline_color(accent()))
        });
    let search_id = search.id();
    let field = field.on_key_down(
        Key::Named(NamedKey::Escape),
        |_| true,
        move |_| {
            filter.set(String::new());
            searching.set(false);
            search_id.request_focus();
        },
    );
    let hint = i18n::label("Find conversation…").style(move |s| {
        s.absolute()
            .inset_left(9)
            .inset_right(6)
            .text_ellipsis()
            .pointer_events_none()
            .font_size(12)
            .color(faint())
            .apply_if(!filter.with(String::is_empty), |s| s.hide())
    });
    let field = Stack::horizontal((field, hint)).style(move |s| {
        s.width_full()
            .items_center()
            .apply_if(compact || !searching.get(), |s| s.hide())
    });
    let section = Stack::horizontal((
        i18n::label("Conversations")
            .style(|s| s.color(muted()).font_size(12).ui_bold()),
        Label::derived(move || {
            wb.history.with(|h| h.conversations.len().to_string())
        })
        .style(|s| s.color(faint()).font_size(11).flex_grow(1_f32)),
        search.tooltip(|| i18n::label("Find a conversation")),
    ))
    .style(move |s| {
        s.width_full()
            .items_center()
            .gap(6)
            .padding_horiz(3)
            .apply_if(compact, |s| s.hide())
    });
    let indexes = move || {
        let query = filter.get().trim().to_lowercase();
        wb.history.with(|h| {
            h.conversations
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, c)| {
                    query.is_empty() || c.title.to_lowercase().contains(&query)
                })
                .map(|(i, c)| (i, c.provider.clone()))
                .collect::<Vec<_>>()
        })
    };
    let tasks = dyn_stack(
        indexes,
        |i| i.clone(),
        move |(i, _)| task_row(wb, i, compact),
    )
    .style(|s| s.flex_col().gap(3).width_full());
    let no_match = i18n::label("No matches").style(move |s| {
        s.color(faint()).font_size(12).padding(6).apply_if(
            compact || filter.with(String::is_empty) || !indexes().is_empty(),
            |s| s.hide(),
        )
    });
    let collapse = Button::new(
        Stack::horizontal((
            icon(Glyph::Rail, 15., muted),
            i18n::label("Collapse sidebar").style(move |s| {
                s.font_size(11)
                    .color(muted())
                    .apply_if(compact, |s| s.hide())
            }),
        ))
        .style(|s| s.items_center().gap(8)),
    )
    .action(toggle)
    .style(move |s| {
        s.width_full()
            .height(32)
            .border(0)
            .background(rail())
            .border_radius(6)
            .hover(|s| s.background(surface()))
            .focus_visible(|s| s.outline(2).outline_color(accent()))
            .apply_if(!compact, |s| s.justify_start().padding_left(7))
    })
    .tooltip(move || {
        if compact {
            tr("Expand sidebar")
        } else {
            tr("Collapse sidebar")
        }
    });
    Stack::vertical((
        brand,
        project_picker(wb, compact),
        new_button,
        Empty::new().style(|s| {
            s.width_full()
                .height(1)
                .background(subtle_line())
                .margin_vert(4)
        }),
        section,
        field,
        Scroll::new(Stack::vertical((tasks, no_match)).style(|s| s.width_full()))
            .style(|s| s.width_full().min_height(0).flex_grow(1_f32)),
        Button::new(
            Stack::horizontal((
                icon(Glyph::Terminal, 15., muted),
                i18n::label("Agent setup").style(move |s| {
                    s.font_size(12).color(ink()).apply_if(compact, |s| s.hide())
                }),
            ))
            .style(|s| s.items_center().gap(8)),
        )
        .action(open_setup)
        .style(move |s| {
            s.width_full()
                .height(30)
                .border(0)
                .border_radius(6)
                .background(rail())
                .apply_if(!compact, |s| s.justify_start().padding_left(7))
                .hover(|s| s.background(surface()))
                .focus_visible(|s| s.outline(2).outline_color(accent()))
        })
        .tooltip(|| i18n::label("Set up coding agents")),
        crate::i18n::picker(compact),
        crate::appearance::picker(wb, compact),
        collapse,
    ))
    .style(move |s| {
        s.width(if compact { 62 } else { 220 })
            .height_full()
            .flex_shrink(0_f32)
            .background(rail())
            .border_right(1)
            .border_color(subtle_line())
            .padding_horiz(if compact { 10 } else { 14 })
            .padding_vert(9)
            .gap(8)
    })
}
