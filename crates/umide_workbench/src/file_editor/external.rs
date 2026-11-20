use std::path::{Path, PathBuf};
use std::rc::Rc;

use floem::{
    ViewId,
    event::{Event, EventListener, EventPropagation},
    ext_event::create_ext_action,
    prelude::*,
    style::TextOverflow,
};

use super::{Confirmation, Entry};
use crate::{
    design::{
        Glyph, accent, button, faint, icon, ink, line, muted, raised, surface,
    },
    editor_picker,
    external_editor::{self, Editor},
};

#[derive(Clone)]
pub(super) enum Target {
    Named(Editor),
    Application(PathBuf),
}

impl Target {
    fn name(&self) -> String {
        match self {
            Self::Named(editor) => editor.label().to_owned(),
            Self::Application(path) => path
                .file_stem()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned(),
        }
    }

    fn launch(&self, root: &Path, relative: &Path) -> Result<(), String> {
        match self {
            Self::Named(editor) => external_editor::launch(*editor, root, relative),
            Self::Application(app) => {
                external_editor::launch_custom(app, root, relative)
            }
        }
    }
}

impl Entry {
    pub(super) fn request_external(self: &Rc<Self>, target: Target) {
        if self.busy.get_untracked() {
            self.external_error.set(Some("The file is busy. Open the editor menu again when the operation finishes.".into()));
            return;
        }
        self.external_error.set(None);
        if self.dirty() {
            self.confirm.set(Some(Confirmation::Open(target)));
        } else {
            self.launch_external(target);
        }
    }

    fn choose_application(self: &Rc<Self>) {
        if self.busy.get_untracked() {
            return;
        }
        // Returning from the native picker can also bring the window into focus.
        // Finish the selection before letting a focus refresh read this buffer.
        self.busy.set(true);
        self.external_error.set(None);
        let entry = self.clone();
        if let Err(error) = editor_picker::choose(self.scope, move |application| {
            entry.busy.set(false);
            if let Some(application) = application {
                entry.request_external(Target::Application(application));
            }
        }) {
            self.busy.set(false);
            self.external_error.set(Some(error));
        }
    }

    pub(super) fn launch_external(self: &Rc<Self>, target: Target) {
        if self.busy.get_untracked() {
            return;
        }
        self.confirm.set(None);
        self.external_error.set(None);
        self.busy.set(true);
        let entry = self.clone();
        let report =
            create_ext_action(self.scope, move |result: Result<(), String>| {
                entry.busy.set(false);
                match result {
                    Ok(()) => entry.external_opened.set(true),
                    Err(error) => entry.external_error.set(Some(error)),
                }
            });
        let workspace = self.workspace.clone();
        let relative = self.path.clone();
        std::thread::spawn(move || {
            report(
                workspace
                    .path()
                    .and_then(|root| target.launch(&root, &relative)),
            );
        });
    }
}

pub(super) fn picker(entry: Rc<Entry>) -> (impl IntoView, ViewId) {
    let busy = entry.busy;
    let control = Button::new(
        Stack::horizontal((
            icon(Glyph::Open, 14., muted),
            "Open in…",
            icon(Glyph::Chevron, 9., faint),
        ))
        .style(|s| s.items_center().gap(5)),
    );
    let id = control.id();
    let control = control
        .action(move || {
            entry.confirm.update(|value| {
                *value = if matches!(value, Some(Confirmation::ChooseEditor)) {
                    None
                } else {
                    Some(Confirmation::ChooseEditor)
                };
            });
        })
        .style(move |s| {
            s.padding_horiz(8)
                .height(30)
                .font_size(12)
                .border(1)
                .border_color(line())
                .border_radius(6)
                .color(ink())
                .background(surface())
                .hover(|s| s.background(raised()))
                .focus_visible(|s| s.outline(2).outline_color(accent()))
                .set_disabled(busy.get())
                .disabled(|s| s.color(faint()))
        })
        .tooltip(|| "Open this file in another editor");
    (control, id)
}

fn choices(entry: Rc<Entry>, picker: ViewId) -> impl IntoView {
    let confirm = entry.confirm;
    let busy = entry.busy;
    let options = Editor::ALL
        .iter()
        .copied()
        .map(Some)
        .chain(std::iter::once(None));
    let buttons: Vec<_> = options
        .map(|option| {
            let entry = entry.clone();
            let label = option.map(Editor::label).unwrap_or("Cancel");
            let row = Button::new(label)
                .action(move || match option {
                    Some(Editor::Custom) => entry.choose_application(),
                    Some(editor) => entry.request_external(Target::Named(editor)),
                    None => {
                        confirm.set(None);
                        picker.request_focus();
                    }
                })
                .style(move |s| {
                    s.height(27)
                        .padding_horiz(8)
                        .font_size(12)
                        .border(1)
                        .border_color(line())
                        .border_radius(5)
                        .background(surface())
                        .color(ink())
                        .hover(|s| s.background(raised()))
                        .focus_visible(|s| s.outline(2).outline_color(accent()))
                        .set_disabled(busy.get())
                });
            (row.id(), row)
        })
        .collect();
    let ids = Rc::new(buttons.iter().map(|(id, _)| *id).collect::<Vec<_>>());
    if let Some(first) = ids.first() {
        first.request_focus();
    }
    let rows = buttons
        .into_iter()
        .enumerate()
        .map(move |(index, (_, row))| {
            let ids = ids.clone();
            row.on_event(EventListener::KeyDown, move |event| {
                if let Event::Key(key) = event
                    && key.state == KeyState::Down
                    && key.modifiers.is_empty()
                {
                    match key.key {
                        Key::Named(NamedKey::Escape) => {
                            confirm.set(None);
                            picker.request_focus();
                        }
                        Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => {
                            ids[(index + ids.len() - 1) % ids.len()].request_focus()
                        }
                        Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => {
                            ids[(index + 1) % ids.len()].request_focus()
                        }
                        _ => return EventPropagation::Continue,
                    }
                    return EventPropagation::Stop;
                }
                EventPropagation::Continue
            })
        });
    Stack::horizontal_from_iter(rows).style(|s| {
        s.width_full()
            .padding(8)
            .gap(5)
            .items_center()
            .flex_wrap(floem::taffy::FlexWrap::Wrap)
            .background(surface())
    })
}

pub(super) fn confirmation(entry: Rc<Entry>, picker: ViewId) -> impl IntoView {
    let confirm = entry.confirm;
    dyn_container(
        move || confirm.get(),
        move |choice| {
            let target = match choice {
                Some(Confirmation::Open(target)) => target,
                Some(Confirmation::ChooseEditor) => {
                    return choices(entry.clone(), picker).into_any();
                }
                _ => return Empty::new().into_any(),
            };
            let save = entry.clone();
            let open = entry.clone();
            let buffer = entry.buffer;
            let save_target = target.clone();
            let title = format!("Open in {} with unsaved edits?", target.name());
            let actions = Stack::horizontal((
                button("Save and open", move || {
                    save.save(Some(save_target.clone()))
                })
                .style(move |s| {
                    s.set_disabled(buffer.with(|b| {
                        b.as_ref().is_none_or(|b| b.recovered_base.is_some())
                    }))
                }),
                button("Open disk version", move || {
                    open.launch_external(target.clone())
                }),
                button("Cancel", move || {
                    confirm.set(None);
                    picker.request_focus();
                }),
            ))
            .style(|s| {
                s.items_center()
                    .gap(5)
                    .flex_wrap(floem::taffy::FlexWrap::Wrap)
            });
            Stack::vertical((
                Label::new(title).style(|s| {
                    s.width_full()
                        .font_size(12)
                        .color(ink())
                        .text_overflow(TextOverflow::Wrap)
                }),
                actions,
            ))
            .style(|s| s.width_full().padding(8).gap(6).background(surface()))
            .into_any()
        },
    )
    .style(move |s| {
        s.width_full().flex_shrink(0_f32).apply_if(
            !confirm.with(|v| {
                matches!(v, Some(Confirmation::ChooseEditor | Confirmation::Open(_)))
            }),
            |s| s.hide(),
        )
    })
}
