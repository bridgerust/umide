use crate::{
    choose_project,
    design::*,
    i18n::{self, tr},
    open_folder, projects,
    state::Workbench,
    worktree,
};
use floem::{prelude::*, style::TextOverflow};
const LINES_PER_PAGE: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    File,
    Hunk,
    Added,
    Removed,
    Context,
    Meta,
}
struct Row {
    text: String,
    kind: Line,
    old: Option<u64>,
    new: Option<u64>,
}

fn hunk_start(s: &str) -> Option<(u64, u64)> {
    let mut parts = s.strip_prefix("@@ ")?.split_ascii_whitespace();
    let number = |s: &str| s.split(',').next()?.parse::<u64>().ok();
    Some((
        number(parts.next()?.strip_prefix('-')?)?,
        number(parts.next()?.strip_prefix('+')?)?,
    ))
}

fn rows(text: &str) -> Vec<Row> {
    let mut position = None;
    let mut untracked = false;
    text.lines()
        .map(|line| {
            let (mut old, mut new) = (None, None);
            let kind = if untracked {
                Line::Meta
            } else if line.starts_with("diff --git ") || line == "Untracked files" {
                untracked = line == "Untracked files";
                position = None;
                Line::File
            } else if let Some(start) = hunk_start(line) {
                position = Some(start);
                Line::Hunk
            } else if let Some((l, r)) = position.as_mut() {
                match line.as_bytes().first() {
                    Some(b'+') => {
                        new = Some(*r);
                        *r = r.saturating_add(1);
                        Line::Added
                    }
                    Some(b'-') => {
                        old = Some(*l);
                        *l = l.saturating_add(1);
                        Line::Removed
                    }
                    Some(b' ') => {
                        old = Some(*l);
                        new = Some(*r);
                        *l = l.saturating_add(1);
                        *r = r.saturating_add(1);
                        Line::Context
                    }
                    _ => Line::Meta,
                }
            } else {
                Line::Meta
            };
            Row {
                text: line.to_owned(),
                kind,
                old,
                new,
            }
        })
        .collect()
}

fn patch_line(row: &Row) -> impl IntoView + use<> {
    let kind = row.kind;
    let foreground = move || match kind {
        Line::File | Line::Context => ink(),
        Line::Hunk => lilac(),
        Line::Added => accent(),
        Line::Removed => danger(),
        Line::Meta => muted(),
    };
    let background = move || match kind {
        Line::File => raised(),
        Line::Hunk => diff_hunk(),
        Line::Added => diff_added(),
        Line::Removed => diff_removed(),
        Line::Context | Line::Meta => panel(),
    };
    let number = |v: Option<u64>| {
        Label::new(v.map(|n| n.to_string()).unwrap_or_default()).style(|s| {
            s.color(faint())
                .width(37)
                .flex_shrink(0_f32)
                .font_size(10)
                .justify_end()
        })
    };
    let label = if kind == Line::File && row.text == "Untracked files" {
        i18n::label("Untracked files")
    } else {
        Label::new(row.text.clone())
    }
    .style(move |s| {
        s.color(foreground())
            .font_size(12)
            .padding_horiz(9)
            .flex_shrink(0_f32)
    });
    Stack::horizontal((number(row.old), number(row.new), label)).style(move |s| {
        s.gap(3)
            .padding_left(4)
            .height(23)
            .min_width_full()
            .items_center()
            .background(background())
            .font_family(MONO.to_owned())
    })
}

fn patch(wb: Workbench, text: String) -> impl IntoView {
    if !text
        .lines()
        .any(|l| l.starts_with("diff --git ") || l == "Untracked files")
    {
        let clean = text == "Working tree is clean.";
        let empty = text.is_empty();
        let color = move || if clean { accent() } else { muted() };
        let symbol = if clean { Glyph::Check } else { Glyph::Branch };
        let open =
            action_button(Glyph::Folder, "Open project", move || choose_project(wb))
                .style(move |s| {
                    s.apply_if(wb.root.with(Option::is_some), |s| s.hide())
                });
        return Stack::vertical((
            icon(symbol, 23., color),
            Label::derived(move || {
                if empty {
                    tr("Open a project to inspect changes.").into()
                } else {
                    i18n::message(&text)
                }
            })
            .style(|s| {
                s.color(muted())
                    .font_size(14)
                    .max_width_full()
                    .text_overflow(TextOverflow::Wrap)
            }),
            open,
        ))
        .style(|s| s.width_full().padding(22).gap(12).items_start())
        .into_any();
    }
    let parsed = rows(&text);
    let count = parsed.len();
    let added = parsed.iter().filter(|r| r.kind == Line::Added).count();
    let removed = parsed.iter().filter(|r| r.kind == Line::Removed).count();
    let summary = Stack::horizontal((
        icon(Glyph::Diff, 14., faint),
        i18n::label("Unified diff").style(|s| s.font_size(12).color(muted())),
        Empty::new().style(|s| s.flex_grow(1_f32)),
        format!("+{added}")
            .style(|s| s.color(accent()).font_family(MONO.to_owned()).font_size(11)),
        format!("−{removed}")
            .style(|s| s.color(danger()).font_family(MONO.to_owned()).font_size(11)),
        crate::button("Copy", move || {
            if let Err(error) = floem::Clipboard::set_contents(text.clone()) {
                wb.status.set(format!("Could not copy diff: {error:?}"));
            }
        }),
    ))
    .style(|s| {
        s.width_full()
            .height(39)
            .gap(9)
            .padding_horiz(10)
            .background(surface())
            .items_center()
            .border_bottom(1)
            .border_color(subtle_line())
    });
    let page = RwSignal::new(0usize);
    let page_count = count.div_ceil(LINES_PER_PAGE);
    let pages = Stack::horizontal((
        Label::derived(move || {
            i18n::fmt(
                "Page {page} of {total}",
                &[
                    ("page", &(page.get() + 1).to_string()),
                    ("total", &page_count.to_string()),
                ],
            )
        })
        .style(|s| s.color(muted()).font_size(12)),
        Empty::new().style(|s| s.flex_grow(1_f32)),
        crate::button("Previous", move || {
            page.update(|p| *p = p.saturating_sub(1))
        })
        .style(move |s| s.set_disabled(page.get() == 0)),
        crate::button("Next", move || {
            page.update(|p| *p = (*p + 1).min(page_count - 1))
        })
        .style(move |s| s.set_disabled(page.get() + 1 == page_count)),
    ))
    .style(move |s| {
        s.width_full()
            .height(38)
            .gap(6)
            .items_center()
            .padding_horiz(10)
            .border_top(1)
            .border_color(subtle_line())
            .apply_if(page_count <= 1, |s| s.hide())
    });
    let body = dyn_container(
        move || page.get(),
        move |current| {
            let offset = current * LINES_PER_PAGE;
            let end = (offset + LINES_PER_PAGE).min(count);
            let lines = Stack::vertical_from_iter(
                parsed[offset..end].iter().map(patch_line),
            )
            .style(|s| s.flex_col().min_width_full());
            Scroll::new(lines).style(|s| s.size_full().min_size(0, 0))
        },
    )
    .style(|s| s.width_full().flex_grow(1_f32).min_size(0, 0));
    Stack::vertical((summary, body, pages))
        .style(|s| {
            s.size_full()
                .min_size(0, 0)
                .background(panel())
                .border(1)
                .border_color(line())
                .border_radius(7)
        })
        .into_any()
}

pub fn changes_view(wb: Workbench) -> impl IntoView {
    let open = action_button(Glyph::Open, "Task folder", move || {
        let Some(project) = wb.root.get_untracked() else {
            return;
        };
        let Some(index) = wb.current.get_untracked() else {
            return;
        };
        let Some(key) = wb.history.with_untracked(|h| {
            h.conversations
                .get(index)
                .and_then(|c| c.worktree_id.clone())
        }) else {
            return;
        };
        let expected = project.clone();
        let report = floem::ext_event::create_ext_action(
            floem::reactive::Scope::current(),
            move |result: Result<(), String>| {
                let Err(error) = result else {
                    return;
                };
                if wb.current.get_untracked() == Some(index)
                    && wb.root.get_untracked().as_ref() == Some(&expected)
                {
                    wb.status.set(error);
                }
            },
        );
        std::thread::spawn(move || {
            let result = projects::data_dir()
                .and_then(|data| worktree::open(&project, &data, &key))
                .and_then(|path| open_folder(&path));
            report(result);
        });
    })
    .style(move |s| {
        s.apply_if(
            !wb.current.get().is_some_and(|i| {
                wb.history.with(|h| {
                    h.conversations
                        .get(i)
                        .is_some_and(|c| c.worktree_id.is_some())
                })
            }),
            |s| s.hide(),
        )
    });
    let subtitle = Label::derived(move || {
        if wb.isolated.get() && wb.current.get().is_some() {
            tr("Changes since this task started")
        } else {
            tr("Working tree and staged files")
        }
    })
    .style(|s| s.font_size(12).color(faint()));
    let header = Stack::vertical((
        i18n::label("Changes").style(|s| s.font_size(25).ui_bold().color(ink())),
        subtitle,
        Stack::horizontal((
            open,
            action_button(Glyph::Refresh, "Refresh", move || wb.show_diff()),
        ))
        .style(|s| s.gap(6).margin_top(5)),
    ))
    .style(|s| s.width_full().gap(4));
    let body = dyn_container(move || wb.changes.get(), move |text| patch(wb, text))
        .style(|s| s.width_full().flex_grow(1_f32).min_size(0, 0));
    Stack::vertical((header, body))
        .style(|s| s.size_full().min_size(0, 0).padding(20).gap(16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_rows_follow_hunks_and_reset_between_files() {
        let patch = r"diff --git a/old b/old
--- a/old
+++ b/old
@@ -12,3 +9,3 @@ function
 context
---minus
+++plus
 context
\ No newline at end of file
@@ -77 +75 @@
-only
+last
diff --git a/new b/new
--- /dev/null
+++ b/new
@@ -0,0 +1 @@
+new

Untracked files
@@ -1 +1 @@
+file";
        let parsed = rows(patch);
        let expect = [
            (Line::File, None, None),
            (Line::Meta, None, None),
            (Line::Meta, None, None),
            (Line::Hunk, None, None),
            (Line::Context, Some(12), Some(9)),
            (Line::Removed, Some(13), None),
            (Line::Added, None, Some(10)),
            (Line::Context, Some(14), Some(11)),
            (Line::Meta, None, None),
            (Line::Hunk, None, None),
            (Line::Removed, Some(77), None),
            (Line::Added, None, Some(75)),
            (Line::File, None, None),
            (Line::Meta, None, None),
            (Line::Meta, None, None),
            (Line::Hunk, None, None),
            (Line::Added, None, Some(1)),
            (Line::Meta, None, None),
            (Line::File, None, None),
            (Line::Meta, None, None),
            (Line::Meta, None, None),
        ];
        assert_eq!(
            parsed
                .iter()
                .map(|r| (r.kind, r.old, r.new))
                .collect::<Vec<_>>(),
            expect,
        );
        assert_eq!(parsed[5].text, "---minus");
        assert_eq!(parsed[20].text, "+file");
    }
}
