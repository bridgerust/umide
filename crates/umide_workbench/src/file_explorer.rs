//! Lazy, workspace-relative file navigation for the native workbench.
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
};

use floem::{
    ViewId,
    event::{Event, EventListener, EventPropagation},
    ext_event::create_ext_action,
    prelude::*,
    reactive::{Memo, Scope},
};

use crate::{
    design::{
        self, Glyph, UiText, accent, faint, icon, ink, mint_wash, muted, raised,
    },
    i18n::{self, fmt, tr},
    project_files::{self, FileEntry},
    state::Workspace,
};

const PAGE_SIZE: usize = 128;

#[derive(Clone, PartialEq, Eq, Hash)]
enum Row {
    Entry(PathBuf, bool),
    More(PathBuf),
    Notice(PathBuf, String),
}

impl Row {
    fn path(&self) -> &Path {
        match self {
            Self::Entry(path, _) | Self::More(path) | Self::Notice(path, _) => path,
        }
    }

    fn indent(&self) -> f64 {
        let depth = self.path().components().count();
        let depth = match self {
            Self::Entry(..) => depth.saturating_sub(1),
            _ => depth,
        };
        4. + 10. * depth.min(6) as f64
    }
}

#[derive(Default)]
struct Model {
    entries: HashMap<PathBuf, Result<Vec<FileEntry>, String>>,
    expanded: HashSet<PathBuf>,
    pending: HashSet<PathBuf>,
    limits: HashMap<PathBuf, usize>,
    generation: u64,
}

impl Model {
    fn append_rows(&self, folder: &Path, rows: &mut Vec<Row>) {
        let entries = match self.entries.get(folder) {
            Some(Ok(entries)) if !entries.is_empty() => entries,
            result => {
                let message = match result {
                    Some(Err(message)) => message.clone(),
                    Some(Ok(_)) => "Empty folder".into(),
                    None => "Loading…".into(),
                };
                rows.push(Row::Notice(folder.to_owned(), message));
                return;
            }
        };
        let limit = self.limits.get(folder).copied().unwrap_or(PAGE_SIZE);
        for entry in entries.iter().take(limit) {
            rows.push(Row::Entry(entry.path.clone(), entry.is_dir));
            if entry.is_dir && self.expanded.contains(&entry.path) {
                self.append_rows(&entry.path, rows);
            }
        }
        if entries.len() > limit {
            rows.push(Row::More(folder.to_owned()));
        }
    }
}

struct Tree {
    workspace: Workspace,
    scope: Scope,
    model: RwSignal<Model>,
    rows: Memo<Vec<Row>>,
    name: RwSignal<String>,
    buttons: RefCell<HashMap<Row, ViewId>>,
    selected: Box<dyn Fn() -> Option<PathBuf>>,
    dirty: Box<dyn Fn(&Path) -> bool>,
    open: Box<dyn Fn(PathBuf)>,
}

impl Tree {
    fn load(&self, folders: Vec<PathBuf>) {
        let workspace = self.workspace.clone();
        let model = self.model;
        let name = self.name;
        let generation = model.with_untracked(|m| m.generation);
        model.update(|m| m.pending.extend(folders.iter().cloned()));
        let receive = create_ext_action(self.scope, move |(root_name, listings)| {
            if model.with_untracked(|m| m.generation) == generation {
                if let Some(root_name) = root_name {
                    name.set(root_name);
                }
                model.update(|m| {
                    for (folder, entries) in listings {
                        m.pending.remove(&folder);
                        m.entries.insert(folder, entries);
                    }
                });
            }
        });
        std::thread::spawn(move || {
            let root = workspace.path();
            let root_name = root.as_ref().ok().map(|r| {
                r.file_name()
                    .unwrap_or(r.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            });
            let listings: Vec<_> = folders
                .into_iter()
                .map(|folder| {
                    let entries = root
                        .as_ref()
                        .map_err(Clone::clone)
                        .and_then(|r| project_files::list(r, &folder));
                    (folder, entries)
                })
                .collect();
            receive((root_name, listings));
        });
    }

    fn refresh(&self) {
        let mut folders = vec![PathBuf::new()];
        self.model.update(|m| {
            m.generation += 1;
            m.pending.clear();
            m.entries
                .retain(|p, _| p.as_os_str().is_empty() || m.expanded.contains(p));
            folders.extend(m.expanded.iter().cloned());
        });
        self.load(folders);
    }

    fn toggle(&self, folder: &Path) {
        let mut load = false;
        self.model.update(|m| {
            if m.expanded.contains(folder) {
                m.expanded.retain(|path| !path.starts_with(folder));
            } else {
                m.expanded.insert(folder.to_owned());
                load =
                    !m.entries.contains_key(folder) && !m.pending.contains(folder);
            }
        });
        if load {
            self.load(vec![folder.to_owned()]);
        }
    }

    fn activate(&self, row: &Row) {
        match row {
            Row::Entry(path, true) => self.toggle(path),
            Row::Entry(path, false) => (self.open)(path.clone()),
            Row::More(path) => self.model.update(|m| {
                *m.limits.entry(path.clone()).or_insert(PAGE_SIZE) += PAGE_SIZE;
            }),
            Row::Notice(..) => {}
        }
    }

    fn navigate(&self, row: &Row, key: &NamedKey) {
        let path = row.path();
        let is_folder = matches!(row, Row::Entry(_, true));
        let expanded =
            is_folder && self.model.with_untracked(|m| m.expanded.contains(path));
        if (expanded && *key == NamedKey::ArrowLeft)
            || (is_folder && !expanded && *key == NamedKey::ArrowRight)
        {
            self.toggle(path);
            return;
        }
        let target = self.rows.with_untracked(|rows| {
            let i = rows.iter().position(|r| r == row)?;
            let focusable = |r: &&Row| !matches!(r, Row::Notice(..));
            match key {
                NamedKey::ArrowDown => rows[i + 1..].iter().find(focusable).cloned(),
                NamedKey::ArrowUp => rows[..i].iter().rev().find(focusable).cloned(),
                NamedKey::Home => rows.iter().find(focusable).cloned(),
                NamedKey::End => rows.iter().rev().find(focusable).cloned(),
                NamedKey::ArrowLeft => {
                    let parent = if matches!(row, Row::More(_)) {
                        path
                    } else {
                        path.parent()?
                    };
                    Some(Row::Entry(parent.to_owned(), true))
                }
                NamedKey::ArrowRight if expanded => rows
                    .get(i + 1)
                    .filter(|child| {
                        matches!(child, Row::Entry(..))
                            && child.path().parent() == Some(path)
                    })
                    .cloned(),
                _ => None,
            }
        });
        if let Some(id) =
            target.and_then(|target| self.buttons.borrow().get(&target).copied())
        {
            id.request_focus();
            id.scroll_to(None);
        }
    }

    fn row(self: &Rc<Self>, row: Row) -> floem::view::AnyView {
        let indent = row.indent();
        if let Row::Notice(_, message) = row {
            let detail = message.clone();
            return Label::derived(move || i18n::message(&detail))
                .style(move |s| {
                    s.width_full()
                        .min_width(0)
                        .text_ellipsis()
                        .padding_left(indent + 13.)
                        .padding_vert(5)
                        .font_size(11)
                        .color(faint())
                })
                .tooltip(move || i18n::message(&message))
                .into_any();
        }
        let folder = matches!(row, Row::Entry(_, true));
        let more = matches!(row, Row::More(_));
        let path = row.path().to_owned();
        let model = self.model;
        let arrow_path = path.clone();
        let arrow = if folder {
            svg(move || {
                if model.with(|m| m.expanded.contains(&arrow_path)) {
                    include_str!("../../../icons/codicons/chevron-down.svg")
                } else {
                    include_str!("../../../icons/codicons/chevron-right.svg")
                }
            })
            .style(|s| s.size(10, 10).flex_shrink(0_f32).color(faint()))
            .into_any()
        } else {
            Empty::new()
                .style(|s| s.size(10, 10).flex_shrink(0_f32))
                .into_any()
        };
        let glyph = if more {
            Glyph::Plus
        } else if folder {
            Glyph::Folder
        } else {
            Glyph::Code
        };
        let label = if more {
            i18n::label("Show more…")
        } else {
            Label::new(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            )
        };
        let label = label.style(|s| {
            s.min_width(0)
                .flex_grow(1_f32)
                .text_ellipsis()
                .font_size(12)
        });
        let tree = self.clone();
        let style_path = path.clone();
        let dot = Empty::new().style(move |s| {
            s.size(5, 5)
                .flex_shrink(0_f32)
                .border_radius(3)
                .background(accent())
                .apply_if(folder || more || !(tree.dirty)(&style_path), |s| s.hide())
        });
        let contents =
            Stack::horizontal((arrow, icon(glyph, 13., muted), label, dot))
                .style(|s| s.width_full().min_width(0).items_center().gap(4));
        let tree = self.clone();
        let action_row = row.clone();
        let style_tree = self.clone();
        let style_path = path.clone();
        let button = Button::new(contents)
            .action(move || tree.activate(&action_row))
            .style(move |s| {
                let selected = !more
                    && !folder
                    && (style_tree.selected)().as_ref() == Some(&style_path);
                s.height(26)
                    .width_full()
                    .min_width(0)
                    .padding_vert(0)
                    .padding_left(indent)
                    .padding_right(5)
                    .border(0)
                    .border_radius(4)
                    .background(if selected {
                        mint_wash()
                    } else {
                        design::panel()
                    })
                    .color(if selected { accent() } else { ink() })
                    .hover(|s| s.background(raised()))
                    .focus_visible(|s| s.outline(1).outline_color(accent()))
            });
        let id = button.id();
        self.buttons.borrow_mut().insert(row.clone(), id);
        let tree = self.clone();
        let nav_row = row.clone();
        let cleanup_tree = self.clone();
        let tooltip_tree = self.clone();
        button
            .on_event(EventListener::KeyDown, move |event| {
                if let Event::Key(key) = event
                    && key.state == KeyState::Down
                    && key.modifiers.is_empty()
                    && let Key::Named(
                        named @ (NamedKey::ArrowUp
                        | NamedKey::ArrowDown
                        | NamedKey::ArrowLeft
                        | NamedKey::ArrowRight
                        | NamedKey::Home
                        | NamedKey::End),
                    ) = &key.key
                {
                    tree.navigate(&nav_row, named);
                    return EventPropagation::Stop;
                }
                EventPropagation::Continue
            })
            .on_cleanup(move || {
                let mut buttons = cleanup_tree.buttons.borrow_mut();
                if buttons.get(&row) == Some(&id) {
                    buttons.remove(&row);
                }
            })
            .tooltip(move || {
                if more {
                    tr("Show more entries").into()
                } else if !folder && (tooltip_tree.dirty)(&path) {
                    fmt(
                        "{path} · Unsaved changes",
                        &[("path", &path.to_string_lossy())],
                    )
                } else {
                    path.display().to_string()
                }
            })
            .into_any()
    }
}

pub(crate) fn panel(
    workspace: Workspace,
    selected: impl Fn() -> Option<PathBuf> + 'static,
    is_dirty: impl Fn(&Path) -> bool + 'static,
    open: impl Fn(PathBuf) + 'static,
) -> impl IntoView {
    let model = RwSignal::new(Model::default());
    let rows = Memo::new(move |_| {
        model.with(|m| {
            let mut rows = Vec::new();
            m.append_rows(Path::new(""), &mut rows);
            rows
        })
    });
    let name = RwSignal::new(String::new());
    let tree = Rc::new(Tree {
        workspace,
        scope: Scope::current(),
        model,
        rows,
        name,
        buttons: RefCell::default(),
        selected: Box::new(selected),
        dirty: Box::new(is_dirty),
        open: Box::new(open),
    });
    tree.load(vec![PathBuf::new()]);
    let reload = tree.clone();
    let refresh = Button::new(icon(Glyph::Refresh, 13., muted))
        .action(move || reload.refresh())
        .style(|s| {
            s.size(24, 24)
                .padding(0)
                .border(0)
                .border_radius(4)
                .flex_shrink(0_f32)
                .background(design::panel())
                .hover(|s| s.background(raised()))
                .focus_visible(|s| s.outline(1).outline_color(accent()))
        })
        .tooltip(|| tr("Refresh files"));
    let header = Stack::horizontal((
        icon(Glyph::Folder, 14., accent),
        Label::derived(move || name.get()).style(|s| {
            s.flex_grow(1_f32)
                .min_width(0)
                .text_ellipsis()
                .font_size(12)
                .ui_bold()
                .color(ink())
        }),
        refresh,
    ))
    .style(|s| {
        s.width_full()
            .height(32)
            .padding_horiz(7)
            .gap(6)
            .items_center()
            .flex_shrink(0_f32)
    });
    let list = dyn_stack(move || rows.get(), Clone::clone, move |row| tree.row(row))
        .style(|s| s.width_full().min_width(0).padding(4).flex_col());
    Stack::vertical((
        header,
        Scroll::new(list).style(|s| s.width_full().min_height(0).flex_grow(1_f32)),
    ))
    .style(|s| {
        s.size_full()
            .min_width(0)
            .min_height(0)
            .background(design::panel())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool) -> FileEntry {
        FileEntry {
            path: path.into(),
            is_dir,
        }
    }

    fn visible(model: &Model) -> Vec<Row> {
        let mut rows = Vec::new();
        model.append_rows(Path::new(""), &mut rows);
        rows
    }

    #[test]
    fn collapsed_subtrees_stay_hidden_while_their_listings_are_cached() {
        let mut model = Model::default();
        model.entries.insert(
            PathBuf::new(),
            Ok(vec![entry("src", true), entry("README", false)]),
        );
        model
            .entries
            .insert("src".into(), Ok(vec![entry("src/lib.rs", false)]));
        assert!(matches!(
            visible(&model).as_slice(),
            [Row::Entry(_, true), Row::Entry(_, false)]
        ));
        model.expanded.insert("src".into());
        assert!(visible(&model).iter().map(Row::path).eq([
            Path::new("src"),
            Path::new("src/lib.rs"),
            Path::new("README")
        ]));
        model.expanded.clear();
        assert_eq!(visible(&model).len(), 2);
    }

    #[test]
    fn large_folders_expose_all_remaining_entries_on_demand() {
        let mut model = Model::default();
        let entries = (0..PAGE_SIZE + 3)
            .map(|i| entry(&format!("file{i}"), false))
            .collect();
        model.entries.insert(PathBuf::new(), Ok(entries));
        let rows = visible(&model);
        assert_eq!(rows.len(), PAGE_SIZE + 1);
        assert!(
            matches!(rows.last(), Some(Row::More(p)) if p.as_os_str().is_empty())
        );
        model.limits.insert(PathBuf::new(), PAGE_SIZE * 2);
        let rows = visible(&model);
        assert_eq!(rows.len(), PAGE_SIZE + 3);
        assert!(rows.iter().all(|r| matches!(r, Row::Entry(..))));
    }
}
