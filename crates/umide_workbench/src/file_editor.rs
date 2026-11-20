//! Native source editor. Documents live with the window, independently of the visible panel.
use std::{
    cell::{Cell, OnceCell},
    collections::HashMap,
    fs,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    rc::Rc,
    time::Instant,
};

use floem::{
    ext_event::create_ext_action,
    prelude::*,
    reactive::Scope,
    style::TextOverflow,
    text::FamilyOwned,
    ui_events::keyboard::Modifiers,
    views::{
        dyn_container,
        editor::{
            command::CommandExecuted,
            keypress::default_key_handler,
            text::{Document, SimpleStyling, WrapMethod},
            text_document::TextDocument,
        },
        text_editor::text_editor_keys,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    design::{button, panel as panel_color, *},
    project_files::{self, TextFile},
    projects,
    state::Workspace,
};

const INACTIVE_FILES: usize = 16;

type Entries = HashMap<(Workspace, PathBuf), Rc<Entry>>;

#[derive(Clone, Copy)]
pub(crate) struct FileEditor {
    scope: Scope,
    entries: RwSignal<Entries>,
    selected: RwSignal<HashMap<Workspace, PathBuf>>,
}

#[derive(Clone)]
struct Buffer {
    doc: Rc<TextDocument>,
    disk: RwSignal<Rc<TextFile>>,
    dirty: RwSignal<bool>,
    // When a recovered draft was based on an older disk version, keep that identity
    // across subsequent launches until the user explicitly reloads the file.
    recovered_base: Option<String>,
}

struct Entry {
    scope: Scope,
    files: FileEditor,
    last_used: Cell<Instant>,
    workspace: Workspace,
    path: PathBuf,
    buffer: RwSignal<Option<Buffer>>,
    busy: RwSignal<bool>,
    message: RwSignal<Option<String>>,
    confirm_reload: RwSignal<bool>,
    recovery_dir: RwSignal<Option<PathBuf>>,
    recovered_record: RwSignal<Option<PathBuf>>,
}

impl FileEditor {
    pub(crate) fn new() -> Self {
        Self {
            scope: Scope::current(),
            entries: RwSignal::new(HashMap::new()),
            selected: RwSignal::new(HashMap::new()),
        }
    }

    fn entry(self, workspace: Workspace, path: PathBuf) -> Rc<Entry> {
        let key = (workspace.clone(), path.clone());
        let entry = self.entries.with_untracked(|all| all.get(&key).cloned());
        if let Some(entry) = entry {
            entry.last_used.set(Instant::now());
            return entry;
        }
        let scope = self.scope.create_child();
        let entry = Rc::new(Entry {
            scope,
            files: self,
            last_used: Cell::new(Instant::now()),
            workspace,
            path,
            buffer: scope.create_rw_signal(None),
            busy: scope.create_rw_signal(false),
            message: scope.create_rw_signal(None),
            confirm_reload: scope.create_rw_signal(false),
            recovery_dir: scope.create_rw_signal(None),
            recovered_record: scope.create_rw_signal(None),
        });
        self.entries.update(|all| {
            all.insert(key, entry.clone());
        });
        entry
    }

    fn trim_cache(self) {
        let mut candidates = self.entries.with_untracked(|all| {
            all.iter()
                // The map is the sole owner only after every mounted view and worker
                // has released the entry. Disposing its scope is then safe.
                .filter(|(_, entry)| {
                    Rc::strong_count(entry) == 1
                        && !entry.busy.get_untracked()
                        && !entry.buffer.with_untracked(|buf| {
                            buf.as_ref().is_some_and(|b| b.dirty.get_untracked())
                        })
                })
                .map(|(key, entry)| (entry.last_used.get(), key.clone()))
                .collect::<Vec<_>>()
        });
        if candidates.len() <= INACTIVE_FILES {
            return;
        }
        candidates.sort_unstable_by_key(|(used, _)| *used);
        let count = candidates.len() - INACTIVE_FILES;
        let removed = self.entries.try_update(|all| {
            candidates
                .into_iter()
                .take(count)
                .filter_map(|(_, key)| all.remove(&key))
                .collect::<Vec<_>>()
        });
        for entry in removed.unwrap_or_default() {
            entry.scope.dispose();
        }
    }

    pub(crate) fn open(self, workspace: Workspace, path: PathBuf) {
        let entry = self.entry(workspace.clone(), path.clone());
        if self
            .selected
            .with_untracked(|s| s.get(&workspace) != Some(&path))
        {
            self.selected.update(|s| {
                s.insert(workspace, path);
            });
        }
        if !entry.dirty() {
            entry.read();
        }
        self.trim_cache();
    }

    pub(crate) fn selected(self, workspace: &Workspace) -> Option<PathBuf> {
        self.selected.with(|s| s.get(workspace).cloned())
    }

    pub(crate) fn is_dirty(self, workspace: &Workspace, path: &Path) -> bool {
        self.entries.with(|all| {
            all.get(&(workspace.clone(), path.to_owned()))
                .is_some_and(|entry| entry.dirty())
        })
    }

    /// Flush unsaved documents to private application storage on normal window close.
    pub(crate) fn close(self) -> Result<(), String> {
        let mut errors = Vec::new();
        self.entries.with_untracked(|all| {
            for entry in all.values() {
                if let Err(error) = entry.persist_draft() {
                    errors.push(format!("{}: {error}", entry.path.display()));
                }
            }
        });
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
}

impl Buffer {
    fn new(scope: Scope, disk: TextFile, draft: Option<&Draft>) -> Self {
        let changed = draft.filter(|draft| draft.text != disk.text);
        let recovered_base = changed
            .filter(|draft| draft.base != disk.fingerprint())
            .map(|draft| draft.base.clone());
        let text = changed.map_or(disk.text.as_str(), |d| d.text.as_str());
        let doc = scope.enter(|| Rc::new(TextDocument::new(scope, text)));
        let dirty = scope.create_rw_signal(changed.is_some());
        let disk = scope.create_rw_signal(Rc::new(disk));
        let weak = Rc::downgrade(&doc);
        doc.add_on_update(move |_| {
            if let Some(doc) = weak.upgrade() {
                let modified =
                    disk.with_untracked(|disk| !matches_text(&doc, &disk.text));
                if modified != dirty.get_untracked() {
                    dirty.set(modified);
                }
            }
        });
        Self {
            doc,
            disk,
            dirty,
            recovered_base,
        }
    }
}

// Compare rope chunks without allocating a full String for every keystroke.
fn matches_text(doc: &TextDocument, text: &str) -> bool {
    let rope = doc.text();
    let mut other = text;
    rope.len() == text.len()
        && rope.iter_chunks(..).all(|chunk| {
            if let Some(rest) = other.strip_prefix(chunk) {
                other = rest;
                true
            } else {
                false
            }
        })
}

impl Entry {
    fn dirty(&self) -> bool {
        self.buffer
            .with(|buf| buf.as_ref().is_some_and(|b| b.dirty.get()))
    }

    fn read(self: &Rc<Self>) {
        if self.busy.get_untracked() {
            return;
        }
        self.confirm_reload.set(false);
        self.busy.set(true);
        let before = self
            .buffer
            .get_untracked()
            .map(|buf| (buf.doc.clone(), buf.doc.text().to_string()));
        let first_open = before.is_none();
        let entry = self.clone();
        let report = create_ext_action(
            self.scope,
            move |result: Result<Opened, String>| {
                entry.busy.set(false);
                entry.files.trim_cache();
                let (disk, dir, recovery) = match result {
                    Ok(opened) => opened,
                    Err(error) => {
                        entry.message.set(Some(error));
                        return;
                    }
                };
                if let Some((doc, previous)) = &before
                    && !matches_text(doc, previous)
                {
                    entry.message.set(Some("You edited this file while it was loading. Your edits are still here.".into()));
                    return;
                }
                entry.recovery_dir.set(Some(dir));
                let draft = match recovery {
                    Ok(record) => record,
                    Err(error) => {
                        entry.message.set(Some(format!(
                            "Could not read the local draft: {error}"
                        )));
                        if first_open {
                            entry.buffer.set(Some(Buffer::new(
                                entry.scope,
                                disk,
                                None,
                            )));
                        }
                        return;
                    }
                };
                let message = draft.as_ref().and_then(|(_, draft)| {
                if draft.text == disk.text { None }
                else if draft.base == disk.fingerprint() { Some("Recovered your unsaved edits.".into()) }
                else { Some("Recovered your edits. The file also changed on disk. Copy the edits you want to keep before reloading.".into()) }
            });
                if let Some((path, _)) = &draft {
                    entry.recovered_record.set(Some(path.clone()));
                }
                if let Some(existing) = entry.buffer.get_untracked()
                    && existing.recovered_base.is_none()
                    && matches_text(&existing.doc, &disk.text)
                {
                    existing.disk.set(Rc::new(disk));
                    existing.dirty.set(false);
                } else {
                    let buffer = Buffer::new(
                        entry.scope,
                        disk,
                        draft.as_ref().map(|(_, d)| d),
                    );
                    entry.buffer.set(Some(buffer));
                }
                entry.message.set(message);
                if !entry.dirty() {
                    entry.remove_recovery();
                }
            },
        );
        let workspace = self.workspace.clone();
        let path = self.path.clone();
        std::thread::spawn(move || {
            let result = workspace.path().and_then(|root| {
                let disk = project_files::open(&root, &path)?;
                let data = projects::data_dir()?;
                let dir = recovery_dir(&data, &root, &path);
                let record = if first_open {
                    latest_draft(&dir)
                } else {
                    Ok(None)
                };
                Ok((disk, dir, record))
            });
            report(result);
        });
    }

    fn save(self: &Rc<Self>) {
        let Some(buf) = self.buffer.get_untracked() else {
            return;
        };
        if self.busy.get_untracked()
            || !buf.dirty.get_untracked()
            || buf.recovered_base.is_some()
        {
            return;
        }
        self.busy.set(true);
        self.confirm_reload.set(false);
        let disk = buf.disk.get_untracked().as_ref().clone();
        let edited = buf.doc.text().to_string();
        let entry = self.clone();
        let report = create_ext_action(
            self.scope,
            move |result: Result<TextFile, String>| {
                entry.busy.set(false);
                entry.files.trim_cache();
                match result {
                    Ok(disk) => {
                        let dirty = !matches_text(&buf.doc, &disk.text);
                        buf.disk.set(Rc::new(disk));
                        buf.dirty.set(dirty);
                        entry.message.set(None);
                        entry.remove_recovery();
                    }
                    Err(error) => entry.message.set(Some(error)),
                }
            },
        );
        std::thread::spawn(move || report(project_files::save(&disk, &edited)));
    }

    fn remove_recovery(&self) {
        if let Some(path) = self.recovered_record.get_untracked() {
            match fs::remove_file(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => {
                    self.message
                        .set(Some(format!("Could not remove the local draft: {e}")));
                }
                _ => self.recovered_record.set(None),
            }
        }
    }

    fn persist_draft(&self) -> io::Result<()> {
        let Some(buf) = self.buffer.get_untracked() else {
            return Ok(());
        };
        if !buf.dirty.get_untracked() {
            self.remove_recovery();
            return Ok(());
        }
        let dir = self
            .recovery_dir
            .get_untracked()
            .ok_or_else(|| io::Error::other("Unknown draft location"))?;
        let draft = Draft {
            relative: self.path.clone(),
            text: buf.doc.text().to_string(),
            base: buf.recovered_base.unwrap_or_else(|| {
                buf.disk.with_untracked(|disk| disk.fingerprint())
            }),
        };
        let path = write_draft(&dir, &draft)?;
        self.remove_recovery();
        self.recovered_record.set(Some(path));
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Draft {
    #[serde(with = "draft_path")]
    pub(crate) relative: PathBuf,
    pub(crate) base: String,
    pub(crate) text: String,
}

// JSON paths normally use strings. Preserve non-Unicode native names losslessly.
pub(crate) mod draft_path {
    use super::*;
    use serde::{Deserializer, Serializer};

    #[derive(Deserialize, Serialize)]
    #[serde(untagged)]
    enum Name {
        Utf8(String),
        #[cfg(unix)]
        Raw(Vec<u8>),
        #[cfg(windows)]
        Wide(Vec<u16>),
    }

    pub(crate) fn serialize<S: Serializer>(
        path: &Path,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        if let Some(text) = path.to_str() {
            return serializer.serialize_str(text);
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Name::Raw(path.as_os_str().as_bytes().into()).serialize(serializer)
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Name::Wide(path.as_os_str().encode_wide().collect())
                .serialize(serializer)
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PathBuf, D::Error> {
        Ok(match Name::deserialize(deserializer)? {
            Name::Utf8(text) => PathBuf::from(text),
            #[cfg(unix)]
            Name::Raw(bytes) => {
                use std::{ffi::OsString, os::unix::ffi::OsStringExt};
                OsString::from_vec(bytes).into()
            }
            #[cfg(windows)]
            Name::Wide(units) => {
                use std::{ffi::OsString, os::windows::ffi::OsStringExt};
                OsString::from_wide(&units).into()
            }
        })
    }
}

type Opened = (TextFile, PathBuf, io::Result<Option<(PathBuf, Draft)>>);

fn path_hash(path: &Path) -> String {
    format!("{:x}", Sha256::digest(path.as_os_str().as_encoded_bytes()))
}

pub(crate) fn workspace_drafts_dir(data: &Path, root: &Path) -> PathBuf {
    data.join("editor-drafts").join(path_hash(root))
}

pub(crate) fn recovery_dir(data: &Path, root: &Path, path: &Path) -> PathBuf {
    workspace_drafts_dir(data, root).join(path_hash(path))
}

pub(crate) fn write_draft(dir: &Path, draft: &Draft) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    let mut writer = BufWriter::new(file.as_file_mut());
    serde_json::to_writer(&mut writer, draft)?;
    writer.flush()?;
    drop(writer);
    file.as_file().sync_all()?;
    // Each window owns its recovery record. Closing another window never overwrites it.
    let path = dir.join(format!("{}.json", uuid::Uuid::new_v4()));
    file.persist_noclobber(&path)?;
    Ok(path)
}

fn latest_draft(dir: &Path) -> io::Result<Option<(PathBuf, Draft)>> {
    let entries = match fs::read_dir(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        result => result?,
    };
    let mut newest = None;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json")
            && entry.file_type()?.is_file()
        {
            let key = (entry.metadata()?.modified()?, path);
            if newest.as_ref().is_none_or(|old| key > *old) {
                newest = Some(key);
            }
        }
    }
    newest
        .map(|(_, path)| {
            let file = fs::File::open(&path)?;
            let draft = serde_json::from_reader(io::BufReader::new(file))?;
            Ok((path, draft))
        })
        .transpose()
}

pub(crate) fn panel(
    files: FileEditor,
    workspace: Workspace,
    leave_editor: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let selection = workspace.clone();
    dyn_container(
        move || files.selected(&selection),
        move |path| match path {
            None => Stack::vertical((
                icon(Glyph::Code, 28., faint),
                "Select a file to edit",
            ))
            .style(|s| {
                s.size_full()
                    .items_center()
                    .justify_center()
                    .gap(10)
                    .color(muted())
            })
            .into_any(),
            Some(path) => {
                let entry = files.entry(workspace.clone(), path);
                if entry.buffer.get_untracked().is_none() {
                    entry.read();
                }
                file_panel(entry, leave_editor).into_any()
            }
        },
    )
    .style(|s| s.size_full().min_size(0, 0).background(panel_color()))
}

fn file_panel(
    entry: Rc<Entry>,
    leave_editor: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let title = entry.path.to_string_lossy().into_owned();
    let dirty = entry.clone();
    let busy = entry.busy;
    let reload = entry.clone();
    let save = entry.clone();
    let can_save = entry.clone();
    let header = Stack::horizontal((
        icon(Glyph::Code, 17., accent).tooltip(|| {
            if cfg!(target_os = "macos") {
                "⌘S to save · Esc to leave the editor"
            } else {
                "Ctrl+S to save · Esc to leave the editor"
            }
        }),
        Label::new(title).style(|s| {
            s.text_ellipsis()
                .font_size(13)
                .ui_bold()
                .min_width(0)
                .flex_grow(1_f32)
                .color(ink())
        }),
        Label::derived(move || if dirty.dirty() { "Edited" } else { "" })
            .style(|s| s.font_size(11).color(accent())),
        action_button(Glyph::Refresh, "Reload", move || {
            if reload.dirty() {
                reload.confirm_reload.set(true);
            } else {
                reload.read();
            }
        })
        .style(move |s| s.set_disabled(busy.get())),
        button("Save", move || save.save()).style(move |s| {
            s.set_disabled(
                busy.get()
                    || !can_save.buffer.with(|buf| {
                        buf.as_ref().is_some_and(|b| {
                            b.dirty.get() && b.recovered_base.is_none()
                        })
                    }),
            )
        }),
    ))
    .style(|s| {
        s.items_center()
            .gap(8)
            .height(44)
            .width_full()
            .padding_horiz(12)
            .flex_shrink(0_f32)
            .border_bottom(1)
            .border_color(subtle_line())
    });

    let confirm = entry.confirm_reload;
    let reload = entry.clone();
    let warning = Stack::horizontal((
        Label::new("Discard unsaved edits and reload this file?").style(|s| {
            s.flex_grow(1_f32)
                .font_size(12)
                .color(ink())
                .text_overflow(TextOverflow::Wrap)
        }),
        button("Cancel", move || confirm.set(false)),
        button("Discard and reload", move || reload.read()),
    ))
    .style(move |s| {
        s.width_full()
            .padding(8)
            .gap(8)
            .items_center()
            .background(surface())
            .apply_if(!confirm.get(), |s| s.hide())
    });
    let message = entry.message;
    let buffer = entry.buffer;
    let clipboard_status = RwSignal::new(None::<String>);
    floem::reactive::Effect::new(move |_| {
        message.track();
        if let Some(buf) = buffer.get() {
            buf.doc.cache_rev().track();
        }
        clipboard_status.set(None);
    });
    let notice = Stack::horizontal((
        Label::derived(move || {
            let mut detail = message.get().unwrap_or_default();
            if let Some(status) = clipboard_status.get() {
                detail.push(' ');
                detail.push_str(&status);
            }
            detail
        })
        .style(|s| {
            s.min_width(0)
                .flex_grow(1_f32)
                .font_size(12)
                .color(muted())
                .text_overflow(TextOverflow::Wrap)
        }),
        button("Copy edits", move || {
            if let Some(buf) = buffer.get_untracked() {
                let result =
                    floem::Clipboard::set_contents(buf.doc.text().to_string());
                clipboard_status.set(Some(match result {
                    Ok(()) => "Edits copied.".into(),
                    Err(floem::ClipboardError::NotAvailable) => {
                        "The clipboard is unavailable.".into()
                    }
                    Err(
                        floem::ClipboardError::ProviderError(detail)
                        | floem::ClipboardError::PathError(detail),
                    ) => format!("Could not copy edits: {detail}"),
                }));
            }
        })
        .style(move |s| {
            s.flex_shrink(0_f32).apply_if(
                !buffer.with(|buf| buf.as_ref().is_some_and(|buf| buf.dirty.get())),
                |s| s.hide(),
            )
        }),
    ))
    .style(move |s| {
        s.width_full()
            .padding_horiz(12)
            .padding_vert(7)
            .gap(8)
            .items_center()
            .background(surface())
            .apply_if(message.get().is_none(), |s| s.hide())
    });
    let content = dyn_container(
        move || buffer.get(),
        move |buf| match buf {
            Some(buf) => editor_view(entry.clone(), buf, leave_editor).into_any(),
            None => {
                Label::derived(move || if busy.get() { "Opening file…" } else { "" })
                    .style(|s| s.padding(14).font_size(12).color(muted()))
                    .into_any()
            }
        },
    )
    .style(|s| s.width_full().flex_grow(1_f32).min_size(0, 0));
    Stack::vertical((header, warning, notice, content))
        .style(|s| s.size_full().min_size(0, 0))
}

fn editor_view(
    entry: Rc<Entry>,
    buf: Buffer,
    leave_editor: impl Fn() + Copy + 'static,
) -> impl IntoView {
    let defaults = OnceCell::new();
    text_editor_keys("", move |ed, key| {
        if key.modifiers.is_empty() && key.key == Key::Named(NamedKey::Escape) {
            leave_editor();
            return CommandExecuted::Yes;
        }
        let modifier = if cfg!(target_os = "macos") {
            Modifiers::META
        } else {
            Modifiers::CONTROL
        };
        if key.modifiers == modifier
            && matches!(&key.key, Key::Character(k) if k.eq_ignore_ascii_case("s"))
        {
            entry.save();
            return CommandExecuted::Yes;
        }
        defaults.get_or_init(|| default_key_handler(ed))(key.clone())
    })
    .use_doc(buf.doc)
    .styling(
        SimpleStyling::builder()
            .font_family(vec![FamilyOwned::Monospace])
            .font_size(13)
            .line_height(1.55)
            .build(),
    )
    .editor_style(|s| {
        s.hide_gutter(false)
            .gutter_left_padding(10.)
            .gutter_right_padding(12.)
            .gutter_dim_color(faint())
            .gutter_accent_color(ink())
            .gutter_current_color(surface())
            .wrap_method(WrapMethod::None)
            .cursor_color(accent())
            .current_line_color(surface())
            .selection_color(mint_wash())
            .indent_guide_color(subtle_line())
    })
    .style(|s| {
        s.size_full()
            .min_size(0, 0)
            .color(ink())
            .background(panel_color())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use floem::views::editor::core::{
        cursor::CursorAffinity, editor::EditType, selection::Selection,
    };

    fn replace(doc: &TextDocument, text: &str) {
        let all = Selection::region(0, doc.text().len(), CursorAffinity::Forward);
        doc.edit(&mut [(all, text)].into_iter(), EditType::Other);
    }

    #[test]
    fn restored_text_stays_dirty_until_it_matches_the_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = Path::new("source.txt");
        let original = "é\n".repeat(4000);
        fs::write(folder.path().join(path), &original).unwrap();
        let disk = project_files::open(folder.path(), path).unwrap();
        let draft = Draft {
            relative: path.into(),
            base: disk.fingerprint(),
            text: original.replace('é', "ê"),
        };
        let scope = Scope::new();
        let buffer = Buffer::new(scope, disk.clone(), Some(&draft));
        assert!(buffer.recovered_base.is_none());
        assert!(buffer.dirty.get_untracked());
        assert_eq!(buffer.doc.text().to_string(), draft.text);
        replace(&buffer.doc, &original);
        assert!(!buffer.dirty.get_untracked());
        replace(&buffer.doc, &draft.text);
        assert!(buffer.dirty.get_untracked());
        assert_eq!(
            fs::read_to_string(folder.path().join(path)).unwrap(),
            original
        );

        // Modifying only the line endings still makes this a different disk baseline.
        fs::write(folder.path().join(path), original.replace('\n', "\r\n")).unwrap();
        let changed = project_files::open(folder.path(), path).unwrap();
        let conflict = Buffer::new(scope, changed, Some(&draft));
        assert_eq!(conflict.recovered_base, Some(draft.base));
        assert_eq!(conflict.doc.text().to_string(), draft.text);
        scope.dispose();
    }

    #[test]
    fn separate_windows_and_workspaces_keep_independent_recovery_files() {
        let data = tempfile::tempdir().unwrap();
        let src = Path::new("src/main.rs");
        let dir = recovery_dir(data.path(), Path::new("/a"), src);
        assert_ne!(dir, recovery_dir(data.path(), Path::new("/b"), src));
        assert!(latest_draft(&dir).unwrap().is_none());

        let first = Draft {
            relative: src.into(),
            base: "first baseline".into(),
            text: "window one".into(),
        };
        let second = Draft {
            relative: src.into(),
            base: "second baseline".into(),
            text: "window two".into(),
        };
        let first_path = write_draft(&dir, &first).unwrap();
        let second_path = write_draft(&dir, &second).unwrap();
        assert_ne!(first_path, second_path);
        let (newest, _) = latest_draft(&dir).unwrap().unwrap();
        fs::remove_file(&newest).unwrap();
        let (other, restored) = latest_draft(&dir).unwrap().unwrap();
        let expected = if other == first_path { first } else { second };
        assert_ne!(newest, other);
        assert_eq!(restored.text, expected.text);
        assert_eq!(restored.base, expected.base);
    }
    #[test]
    fn closing_writes_only_dirty_documents_and_never_touches_source_files() {
        let scope = Scope::new();
        let project = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let wb = scope.enter(crate::state::Workbench::new);
        wb.root.set(Some(project.path().to_path_buf()));
        wb.isolated.set(false);
        let workspace = wb.selected_workspace().unwrap();
        let files = scope.enter(FileEditor::new);
        let load = |name: &str| {
            let path = Path::new(name);
            fs::write(project.path().join(path), "disk version").unwrap();
            let entry = files.entry(workspace.clone(), path.into());
            let disk = project_files::open(project.path(), path).unwrap();
            let buffer = Buffer::new(scope, disk, None);
            entry.buffer.set(Some(buffer.clone()));
            let storage = recovery_dir(data.path(), project.path(), path);
            entry.recovery_dir.set(Some(storage.clone()));
            (entry, buffer, storage)
        };
        let (edited_entry, edited, edited_storage) = load("edited.txt");
        let (_, _, clean_storage) = load("clean.txt");
        replace(&edited.doc, "unsaved draft");
        assert!(files.is_dirty(&workspace, Path::new("edited.txt")));
        files.close().unwrap();

        let (record, draft) = latest_draft(&edited_storage).unwrap().unwrap();
        assert_eq!(draft.text, "unsaved draft");
        assert_eq!(draft.base, edited.disk.get_untracked().fingerprint());
        assert_eq!(
            fs::read_to_string(project.path().join("edited.txt")).unwrap(),
            "disk version"
        );
        assert!(latest_draft(&clean_storage).unwrap().is_none());
        // Returning to the disk contents removes only this document's recovery record.
        replace(&edited.doc, "disk version");
        files.close().unwrap();
        assert!(!record.exists());
        assert!(edited_entry.recovered_record.get_untracked().is_none());
        scope.dispose();
    }
    #[test]
    fn eviction_retains_edits_mounted_views_and_active_operations() {
        let scope = Scope::new();
        let project = tempfile::tempdir().unwrap();
        let wb = scope.enter(crate::state::Workbench::new);
        wb.root.set(Some(project.path().to_path_buf()));
        wb.isolated.set(false);
        let ws = wb.selected_workspace().unwrap();
        let files = scope.enter(FileEditor::new);

        let dirty = files.entry(ws.clone(), "edited.txt".into());
        fs::write(project.path().join("edited.txt"), "original").unwrap();
        let disk = project_files::open(project.path(), &dirty.path).unwrap();
        let buf = Buffer::new(dirty.scope, disk, None);
        replace(&buf.doc, "draft");
        dirty.buffer.set(Some(buf));
        drop(dirty);
        let mounted = files.entry(ws.clone(), "visible.txt".into());
        files.entry(ws.clone(), "loading.txt".into()).busy.set(true);
        for i in 0..(INACTIVE_FILES * 2) {
            files.entry(ws.clone(), PathBuf::from(i.to_string()));
        }

        files.trim_cache();
        files.entries.with_untracked(|all| {
            assert_eq!(all.len(), INACTIVE_FILES + 3);
            assert!(all.contains_key(&(ws.clone(), "loading.txt".into())));
            assert!(all.contains_key(&(ws.clone(), mounted.path.clone())));
            assert!(!all.contains_key(&(ws.clone(), "0".into())));
            assert!(all.contains_key(&(
                ws.clone(),
                (INACTIVE_FILES * 2 - 1).to_string().into()
            )));
        });
        assert!(files.is_dirty(&ws, Path::new("edited.txt")));
        scope.dispose();
    }

    #[cfg(unix)]
    #[test]
    fn recovery_record_preserves_non_unicode_filenames() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let relative: PathBuf =
            OsString::from_vec(b"invalid-\xff.txt".to_vec()).into();
        let draft = Draft {
            relative: relative.clone(),
            base: String::new(),
            text: "edits".into(),
        };
        let raw = serde_json::to_vec(&draft).unwrap();
        let restored: Draft = serde_json::from_slice(&raw).unwrap();
        assert_eq!(restored.relative, relative);
    }
}
