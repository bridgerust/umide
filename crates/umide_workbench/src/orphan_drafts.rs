//! Recover unsaved text when its original workspace file is no longer editable.
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, BufReader},
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

use floem::{
    Clipboard, ext_event::create_ext_action, prelude::*, reactive::Scope,
    style::TextOverflow,
};
use serde::Deserialize;

use crate::{
    design::{UiText, accent, button, ink, muted, subtle_line, surface},
    file_editor::{Draft, recovery_dir, workspace_drafts_dir},
    project_files, projects,
    state::Workspace,
};

#[derive(Clone)]
struct Record {
    relative: PathBuf,
    stored: PathBuf,
    modified: SystemTime,
    version: Option<usize>,
}

#[derive(Clone, Default)]
struct Scan {
    records: Vec<Record>,
    problem: Option<String>,
}

#[derive(Deserialize)]
struct RecordHeader {
    #[serde(with = "crate::file_editor::draft_path")]
    relative: PathBuf,
}

fn read_record<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    if !path.symlink_metadata()?.file_type().is_file() {
        return Err(io::Error::other("The draft is no longer a regular file."));
    }
    Ok(serde_json::from_reader(BufReader::new(File::open(path)?))?)
}

fn scan(data: &Path, root: &Path) -> io::Result<Scan> {
    let workspace = workspace_drafts_dir(data, root);
    match workspace.symlink_metadata() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Scan::default()),
        Ok(meta) if !meta.is_dir() => {
            return Err(io::Error::other("Invalid draft folder."));
        }
        result => {
            result?;
        }
    }
    let mut scan = Scan::default();
    let mut editable = HashMap::new();
    for bucket in fs::read_dir(&workspace)? {
        let bucket = match bucket.and_then(|entry| Ok((entry.file_type()?, entry))) {
            Ok((kind, entry)) if kind.is_dir() => entry,
            Ok(_) => continue,
            Err(_) => {
                scan.problem = Some("Some saved drafts could not be read.".into());
                continue;
            }
        };
        let inspect = |file: fs::DirEntry| -> io::Result<Option<Record>> {
            let stored = file.path();
            if !file.file_type()?.is_file()
                || stored.extension().is_none_or(|ext| ext != "json")
            {
                return Ok(None);
            }
            let RecordHeader { relative } = read_record(&stored)?;
            if relative.as_os_str().is_empty()
                || !relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)))
                || recovery_dir(data, root, &relative) != bucket.path()
            {
                return Err(io::Error::other("Invalid draft path."));
            }
            Ok(Some(Record {
                relative,
                stored,
                modified: file.metadata()?.modified()?,
                version: None,
            }))
        };
        let files = match fs::read_dir(bucket.path()) {
            Ok(files) => files,
            Err(_) => {
                scan.problem = Some("Some saved drafts could not be read.".into());
                continue;
            }
        };
        for item in files {
            match item.and_then(&inspect) {
                Ok(Some(record)) => {
                    let can_open =
                        editable.entry(record.relative.clone()).or_insert_with(
                            || project_files::open(root, &record.relative).is_ok(),
                        );
                    if !*can_open {
                        scan.records.push(record);
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    scan.problem =
                        Some("Some saved drafts could not be read.".into())
                }
            }
        }
    }
    scan.records.sort_by(|a, b| {
        a.relative
            .cmp(&b.relative)
            .then_with(|| b.modified.cmp(&a.modified))
            .then_with(|| b.stored.cmp(&a.stored))
    });
    for records in scan.records.chunk_by_mut(|a, b| a.relative == b.relative) {
        if records.len() > 1 {
            // Newest first. Number versions chronologically for display.
            let count = records.len();
            for (i, record) in records.iter_mut().enumerate() {
                record.version = Some(count - i);
            }
        }
    }
    Ok(scan)
}

fn recover(record: &Record) -> io::Result<String> {
    let draft: Draft = read_record(&record.stored)?;
    if draft.relative != record.relative {
        return Err(io::Error::other("The saved draft has changed."));
    }
    Ok(draft.text)
}

pub(crate) fn panel(workspace: Workspace) -> impl IntoView {
    let scope = Scope::current();
    let scan_result = RwSignal::new(Scan::default());
    let copying = RwSignal::new(false);
    let status = RwSignal::new(None::<String>);
    let receive = create_ext_action(scope, move |scan| scan_result.set(scan));
    std::thread::spawn(move || {
        let result = workspace.path().and_then(|root| {
            scan(&projects::data_dir()?, &root).map_err(|e| e.to_string())
        });
        receive(result.unwrap_or_else(|e| Scan {
            problem: Some(format!("Could not check saved drafts: {e}")),
            records: vec![],
        }));
    });
    let header = "Drafts with unavailable files"
        .style(|s| s.font_size(12).ui_bold().color(ink()));
    let rows = dyn_stack(
        move || scan_result.with(|s| s.records.clone()),
        |r| r.stored.clone(),
        move |record| {
            let relative = record.relative.display().to_string();
            let title = match record.version {
                Some(n) => format!("{relative} · Version {n}"),
                None => relative.clone(),
            };
            let label = Label::new(title.clone())
                .style(|s| {
                    s.min_width(0)
                        .text_ellipsis()
                        .flex_grow(1_f32)
                        .font_size(12)
                        .color(muted())
                })
                .tooltip(move || title.clone());
            let copy = button("Copy edits", move || {
                if copying.get_untracked() {
                    return;
                }
                copying.set(true);
                status.set(None);
                let name = relative.clone();
                let report =
                    create_ext_action(scope, move |text: io::Result<String>| {
                        let message = match text {
                            Ok(text) => match Clipboard::set_contents(text) {
                                Ok(()) => format!("Copied edits for {name}."),
                                Err(_) => {
                                    "Could not copy the draft to the clipboard."
                                        .into()
                                }
                            },
                            Err(e) => format!("Could not recover the draft: {e}"),
                        };
                        copying.set(false);
                        status.set(Some(message));
                    });
                let record = record.clone();
                std::thread::spawn(move || report(recover(&record)));
            })
            .style(move |s| s.set_disabled(copying.get()).flex_shrink(0_f32));
            Stack::horizontal((label, copy))
                .style(|s| s.width_full().height(31).gap(8).items_center())
        },
    )
    .style(|s| s.width_full().flex_col());
    let scroll = Scroll::new(rows).style(move |s| {
        let count = scan_result.with(|scan| scan.records.len());
        s.width_full()
            .height((count.min(3) * 31) as f64)
            .apply_if(count == 0, |s| s.hide())
    });
    let message = move || {
        status
            .get()
            .or_else(|| scan_result.with(|s| s.problem.clone()))
    };
    let notice =
        Label::derived(move || message().unwrap_or_default()).style(move |s| {
            s.width_full()
                .font_size(11)
                .color(accent())
                .text_overflow(TextOverflow::Wrap)
                .apply_if(message().is_none(), |s| s.hide())
        });
    Stack::vertical((header, scroll, notice)).style(move |s| {
        s.width_full()
            .min_width(0)
            .flex_shrink(0_f32)
            .padding_horiz(10)
            .padding_vert(5)
            .gap(3)
            .border_bottom(1)
            .border_color(subtle_line())
            .background(surface())
            .apply_if(
                scan_result.with(|s| s.records.is_empty() && s.problem.is_none()),
                |s| s.hide(),
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_editor::write_draft;

    #[test]
    fn recovers_edits_after_rename_only_from_the_active_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let project = tmp.path().join("project");
        let other = tmp.path().join("other");
        let path = Path::new("src/main.rs");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::create_dir_all(other.join("src")).unwrap();
        fs::write(project.join(path), "original").unwrap();
        for (root, text) in [
            (&project, "first edits"),
            (&project, "second edits"),
            (&other, "other edits"),
        ] {
            write_draft(
                &recovery_dir(&data, root, path),
                &Draft {
                    relative: path.to_owned(),
                    base: String::new(),
                    text: text.into(),
                },
            )
            .unwrap();
        }
        assert!(scan(&data, &project).unwrap().records.is_empty());
        fs::rename(project.join(path), project.join("src/renamed.rs")).unwrap();
        let found = scan(&data, &project).unwrap();
        assert_eq!(found.records.len(), 2);
        assert!(
            found
                .records
                .iter()
                .all(|r| r.relative == path && r.version.is_some())
        );
        let mut texts: Vec<_> =
            found.records.iter().map(|r| recover(r).unwrap()).collect();
        texts.sort();
        assert_eq!(texts, ["first edits", "second edits"]);
        assert_eq!(
            fs::read_to_string(project.join("src/renamed.rs")).unwrap(),
            "original"
        );
    }
    fn assert_unavailable_draft(path: &Path, disk: Option<&[u8]>) {
        let folder = tempfile::tempdir().unwrap();
        let project = folder.path().join("project");
        let data = folder.path().join("data");
        fs::create_dir(&project).unwrap();
        if let Some(bytes) = disk {
            fs::write(project.join(path), bytes).unwrap();
        }
        write_draft(
            &recovery_dir(&data, &project, path),
            &Draft {
                relative: path.to_owned(),
                base: String::new(),
                text: "local edits".into(),
            },
        )
        .unwrap();
        let found = scan(&data, &project).unwrap();
        assert!(found.problem.is_none());
        assert_eq!(found.records.len(), 1);
        assert_eq!(found.records[0].relative, path);
        assert_eq!(recover(&found.records[0]).unwrap(), "local edits");
    }

    #[test]
    fn recovers_text_after_a_file_becomes_binary() {
        assert_unavailable_draft(Path::new("replaced.txt"), Some(&[0, 255, 0]));
    }

    #[cfg(unix)]
    #[test]
    fn preserves_non_unicode_names_when_the_source_is_missing() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
        assert_unavailable_draft(
            Path::new(OsStr::from_bytes(b"unusual-\xff-name")),
            None,
        );
    }
}
