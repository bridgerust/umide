use floem::{prelude::*, reactive::Memo, style::TextOverflow};

use crate::{
    choose_project,
    design::{Glyph, UiText, accent, action_button, ink, muted, subtle_line},
    file_editor::{self, FileEditor},
    file_explorer, orphan_drafts,
    state::{Workbench, Workspace},
};

fn workspace_files(
    wb: Workbench,
    files: FileEditor,
    workspace: Workspace,
    leave_editor: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let selected_workspace = workspace.clone();
    let selected = Memo::new(move |_| files.selected(&selected_workspace));
    let browsing = RwSignal::new(selected.get_untracked().is_none());
    let split = RwSignal::new(false);
    let dirty_workspace = workspace.clone();
    let open_workspace = workspace.clone();
    let explorer = file_explorer::panel(
        workspace.clone(),
        move || selected.get(),
        move |path| files.is_dirty(&dirty_workspace, path),
        move |path| {
            files.open(open_workspace.clone(), path);
            browsing.set(false);
        },
    )
    .style(move |s| {
        s.height_full()
            .min_size(0, 0)
            .apply_if(split.get(), |s| {
                s.width(186)
                    .flex_shrink(0_f32)
                    .border_right(1)
                    .border_color(subtle_line())
            })
            .apply_if(!split.get(), |s| {
                s.width_full().apply_if(!browsing.get(), |s| s.hide())
            })
    });
    let recoveries = orphan_drafts::panel(workspace.clone());
    let editor =
        file_editor::panel(files, workspace, leave_editor).style(move |s| {
            s.height_full()
                .min_size(0, 0)
                .flex_basis(0)
                .flex_grow(1_f32)
                .apply_if(!split.get() && browsing.get(), |s| s.hide())
        });
    let browse =
        action_button(Glyph::Folder, "Browse files", move || browsing.set(true))
            .style(move |s| s.apply_if(split.get() || browsing.get(), |s| s.hide()));
    let resume =
        action_button(Glyph::Code, "Back to file", move || browsing.set(false))
            .style(move |s| {
                s.apply_if(
                    split.get() || !browsing.get() || selected.get().is_none(),
                    |s| s.hide(),
                )
            });
    let scope = Label::derived(move || {
        if wb.isolated.get() {
            "Isolated Git copy"
        } else {
            "Project folder"
        }
    })
    .style(|s| s.font_size(11).color(muted()));
    let title = Stack::vertical((
        "Project files".style(|s| s.font_size(18).ui_bold().color(ink())),
        scope,
    ))
    .style(|s| s.gap(2).flex_grow(1_f32));
    let header = Stack::horizontal((title, browse, resume)).style(|s| {
        s.width_full()
            .items_center()
            .height(61)
            .flex_shrink(0_f32)
            .padding_horiz(12)
            .gap(6)
            .border_bottom(1)
            .border_color(subtle_line())
    });
    let body = Stack::horizontal((explorer, editor))
        .style(|s| s.width_full().min_size(0, 0).flex_grow(1_f32));
    Stack::vertical((header, recoveries, body))
        .style(|s| s.size_full().min_size(0, 0))
        .on_resize(move |rect| {
            let side_by_side = rect.width() >= 620.;
            if side_by_side != split.get_untracked() {
                split.set(side_by_side);
            }
        })
}

pub(crate) fn panel(
    wb: Workbench,
    files: FileEditor,
    leave_editor: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let selected = wb.workspace_selection();
    dyn_container(
        move || selected.get(),
        move |selection| match selection {
            Ok(workspace) => {
                workspace_files(wb, files, workspace, leave_editor).into_any()
            }
            Err(detail) => {
                let needs_project = wb.root.with_untracked(Option::is_none);
                Stack::vertical((
                    crate::design::icon(Glyph::Folder, 26., accent),
                    "Project files"
                        .style(|s| s.font_size(18).ui_bold().color(ink())),
                    Label::new(detail).style(|s| {
                        s.width_full()
                            .font_size(13)
                            .color(muted())
                            .text_overflow(TextOverflow::Wrap)
                    }),
                    action_button(Glyph::Open, "Open project…", move || {
                        choose_project(wb)
                    })
                    .style(move |s| s.apply_if(!needs_project, |s| s.hide())),
                ))
                .style(|s| s.width_full().padding(16).items_start().gap(10))
                .into_any()
            }
        },
    )
}
