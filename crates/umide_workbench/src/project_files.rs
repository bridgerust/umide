//! Workspace-relative file browsing and optimistic saves for the native editor.
use std::{
    borrow::Cow,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

const MAX_BYTES: usize = 2 * 1024 * 1024;
const BOM: &[u8] = b"\xef\xbb\xbf";
// All editor tabs and aliases share this check-and-replace critical section.
static SAVES: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Clone, Debug)]
pub struct TextFile {
    pub text: String,
    root: PathBuf,
    relative: PathBuf,
    target: PathBuf,
    digest: [u8; 32],
    bom: bool,
    crlf: Vec<bool>,
}

fn git_component(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str().to_str().is_some_and(|s| {
            s.trim_end_matches([' ', '.']).eq_ignore_ascii_case(".git")
        })
    })
}

fn workspace_root(root: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("Could not open workspace: {e}"))?;
    if !root.is_dir() {
        return Err("The workspace must be a folder.".into());
    }
    Ok(root)
}

fn resolve(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        || git_component(relative)
        || git_component(root)
    {
        return Err(
            "Choose a file inside this workspace, outside Git’s internal directory."
                .into(),
        );
    }
    let target = root.join(relative).canonicalize().map_err(|e| {
        format!("Could not open {}: {e}", root.join(relative).display())
    })?;
    if !target.starts_with(root) || git_component(&target) {
        return Err("This path points outside the editable workspace.".into());
    }
    Ok(target)
}

/// Resolve a file for another editor without imposing our text encoding or size limits.
pub(crate) fn external_target(
    root: &Path,
    relative: &Path,
) -> Result<(PathBuf, PathBuf), String> {
    let root = workspace_root(root)?;
    let target = resolve(&root, relative)?;
    if !target.is_file() {
        return Err("Choose a regular file in this workspace.".into());
    }
    Ok((root, target))
}

pub fn list(root: &Path, relative: &Path) -> Result<Vec<FileEntry>, String> {
    let root = workspace_root(root)?;
    let directory = resolve(&root, relative)?;
    let children = directory
        .read_dir()
        .map_err(|e| format!("Could not list {}: {e}", directory.display()))?;
    let mut entries = Vec::new();
    for child in children {
        let child = child
            .map_err(|e| format!("Could not list {}: {e}", directory.display()))?;
        let path = relative.join(child.file_name());
        if git_component(&path) {
            continue;
        }
        let kind = child
            .file_type()
            .map_err(|e| format!("Could not inspect {}: {e}", path.display()))?;
        let kind = if kind.is_symlink() {
            // In-tree file aliases are editable; skip broken links, escaping targets
            // and links back to a directory which is already on this branch.
            let Ok(real) = resolve(&root, &path) else {
                continue;
            };
            if directory.starts_with(&real) {
                continue;
            }
            let Ok(metadata) = real.metadata() else {
                continue;
            };
            metadata.file_type()
        } else {
            kind
        };
        if kind.is_file() || kind.is_dir() {
            entries.push(FileEntry {
                path,
                is_dir: kind.is_dir(),
            });
        }
    }
    entries.sort_by_cached_key(|entry| {
        (
            !entry.is_dir,
            entry
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase(),
            entry.path.clone(),
        )
    });
    Ok(entries)
}

fn bytes(path: &Path) -> Result<(Vec<u8>, fs::Permissions), String> {
    let read_error = |e| format!("Could not read {}: {e}", path.display());
    if !path.metadata().map_err(read_error)?.is_file() {
        return Err("Choose a regular text file.".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file: File = options.open(path).map_err(read_error)?;
    let meta = file.metadata().map_err(read_error)?;
    if !meta.is_file() {
        return Err("Choose a regular text file.".into());
    }
    if meta.len() > MAX_BYTES as u64 {
        return Err("This file is too large for the editor (2 MiB limit).".into());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(read_error)?;
    if bytes.len() > MAX_BYTES {
        return Err("This file is too large for the editor (2 MiB limit).".into());
    }
    Ok((bytes, meta.permissions()))
}

impl TextFile {
    pub fn fingerprint(&self) -> String {
        use std::fmt::Write;
        self.digest
            .iter()
            .fold(String::with_capacity(64), |mut hex, byte| {
                write!(hex, "{byte:02x}").expect("writing to a string");
                hex
            })
    }

    fn decode(
        root: PathBuf,
        relative: PathBuf,
        target: PathBuf,
        raw: &[u8],
    ) -> Result<Self, String> {
        let bom = raw.starts_with(BOM);
        let source = std::str::from_utf8(raw.strip_prefix(BOM).unwrap_or(raw))
            .map_err(
                |_| "This file is binary or uses an encoding other than UTF-8.",
            )?;
        if source
            .bytes()
            .any(|b| b < 0x20 && !matches!(b, b'\t' | b'\r' | b'\n' | 0x0c))
        {
            return Err("This appears to be a binary file. The editor opens UTF-8 text files.".into());
        }
        let crlf = source
            .split_inclusive('\n')
            .filter(|line| line.ends_with('\n'))
            .map(|line| line.ends_with("\r\n"))
            .collect();
        Ok(Self {
            text: source.replace("\r\n", "\n"),
            root,
            relative,
            target,
            digest: Sha256::digest(raw).into(),
            bom,
            crlf,
        })
    }

    fn encode(&self, edited: &str) -> Result<Vec<u8>, String> {
        if edited.len() > MAX_BYTES {
            return Err("Edited file exceeds the 2 MiB limit.".into());
        }
        let edited = if edited.contains("\r\n") {
            Cow::Owned(edited.replace("\r\n", "\n"))
        } else {
            Cow::Borrowed(edited)
        };
        let new_count = edited.bytes().filter(|&b| b == b'\n').count();
        let old_count = self.crlf.len();
        let prefix = self
            .text
            .split_inclusive('\n')
            .zip(edited.split_inclusive('\n'))
            .take_while(|(a, b)| a == b && a.ends_with('\n'))
            .count()
            .min(old_count.min(new_count));
        // Match unchanged trailing lines even when a paste inserts or removes lines
        // above them. This keeps the original terminators in mixed-ending files.
        let suffix = self
            .text
            .split_inclusive('\n')
            .rev()
            .zip(edited.split_inclusive('\n').rev())
            .take_while(|(a, b)| a == b)
            .filter(|(a, _)| a.ends_with('\n'))
            .count()
            .min(old_count.min(new_count) - prefix);
        let cr_count = self.crlf.iter().filter(|&&cr| cr).count();
        let default_crlf = match cr_count.cmp(&(old_count - cr_count)) {
            std::cmp::Ordering::Equal => self.crlf.first().copied().unwrap_or(false),
            order => order.is_gt(),
        };
        let mut raw = Vec::with_capacity(edited.len());
        if self.bom {
            raw.extend_from_slice(BOM);
        }
        for (i, line) in edited.split_inclusive('\n').enumerate() {
            if let Some(content) = line.strip_suffix('\n') {
                raw.extend_from_slice(content.as_bytes());
                let cr = if i >= new_count - suffix {
                    self.crlf[old_count - (new_count - i)]
                } else if i < old_count - suffix {
                    self.crlf[i]
                } else {
                    default_crlf
                };
                if cr {
                    raw.push(b'\r');
                }
                raw.push(b'\n');
            } else {
                raw.extend_from_slice(line.as_bytes());
            }
        }
        if raw.len() > MAX_BYTES {
            return Err("Edited file exceeds the 2 MiB limit.".into());
        }
        Ok(raw)
    }
}

pub fn open(root: &Path, relative: &Path) -> Result<TextFile, String> {
    let root = workspace_root(root)?;
    let target = resolve(&root, relative)?;
    let (raw, _) = bytes(&target)?;
    TextFile::decode(root, relative.to_owned(), target, &raw)
}

pub fn save(file: &TextFile, edited: &str) -> Result<TextFile, String> {
    let raw = file.encode(edited)?;
    let updated = TextFile::decode(
        file.root.clone(),
        file.relative.clone(),
        file.target.clone(),
        &raw,
    )?;
    let write_error = |e| format!("Could not save {}: {e}", file.relative.display());
    let _guard = SAVES.lock().unwrap_or_else(|e| e.into_inner());
    let parent = file.target.parent().ok_or("Invalid workspace file.")?;
    if resolve(&file.root, &file.relative)? != file.target {
        return Err(
            "The file now points to a different location. Reload before saving."
                .into(),
        );
    }
    // Prepare changes alongside the target. A failed write leaves the original
    // intact. Open and compare the baseline immediately before the atomic rename.
    let mut temp = NamedTempFile::new_in(parent).map_err(write_error)?;
    temp.write_all(&raw).map_err(write_error)?;
    temp.as_file().sync_all().map_err(write_error)?;
    if resolve(&file.root, &file.relative)? != file.target {
        return Err(
            "The file now points to a different location. Reload before saving."
                .into(),
        );
    }
    let (current, permissions) = bytes(&file.target)?;
    let current_digest: [u8; 32] = Sha256::digest(&current).into();
    if updated.digest == current_digest {
        return Ok(updated);
    }
    if current_digest != file.digest {
        return Err("The file changed on disk. Your edits are still open. Reload to use the disk version.".into());
    }
    if permissions.readonly() {
        return Err("The file is read-only. Your edits are still open.".into());
    }
    temp.as_file()
        .set_permissions(permissions)
        .map_err(write_error)?;
    temp.persist(&file.target)
        .map_err(|e| write_error(e.error))?;
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use tempfile::tempdir;

    #[test]
    fn directory_listing_is_lazy_sorted_and_keeps_project_dotfiles() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        for folder in ["src", "Assets", ".git"] {
            fs::create_dir(root.join(folder)).unwrap();
        }
        for name in [
            ".gitignore",
            "main.rs",
            "Cargo.toml",
            "src/lib.rs",
            ".git/config",
        ] {
            fs::write(root.join(name), "").unwrap();
        }
        let entries = list(root, Path::new("")).unwrap();
        assert_eq!(
            entries
                .into_iter()
                .map(|e| (e.path, e.is_dir))
                .collect::<Vec<_>>(),
            [
                ("Assets", true),
                ("src", true),
                (".gitignore", false),
                ("Cargo.toml", false),
                ("main.rs", false)
            ]
            .map(|(name, folder)| (PathBuf::from(name), folder))
        );
        assert_eq!(
            list(root, Path::new("src")).unwrap()[0].path,
            Path::new("src/lib.rs")
        );
        assert!(list(root, Path::new(".git")).is_err());
        assert!(open(root, Path::new(".git/config")).is_err());
        assert!(open(root, &root.join("main.rs")).is_err());
        assert!(list(root, Path::new("src/..")).is_err());
    }

    #[test]
    fn saves_preserve_encoding_line_endings_and_executable_bits() {
        let dir = tempdir().unwrap();
        let name = Path::new("script");
        let path = dir.path().join(name);
        fs::write(&path, b"\xef\xbb\xbf#!shell\r\nvalue\r\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
        }
        let file = open(dir.path(), name).unwrap();
        assert_eq!(file.text, "#!shell\nvalue\n");
        let updated = save(&file, "#!shell\nnew\nwith paste\r\n").unwrap();
        assert_eq!(updated.text, "#!shell\nnew\nwith paste\n");
        assert_eq!(
            fs::read(&path).unwrap(),
            b"\xef\xbb\xbf#!shell\r\nnew\r\nwith paste\r\n"
        );
        save(&updated, "").unwrap();
        assert_eq!(fs::read(&path).unwrap(), BOM);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o750
            );
        }
    }

    #[test]
    fn preserves_existing_mixed_endings_during_insertions_and_deletions() {
        let dir = tempdir().unwrap();
        let name = Path::new("mixed");
        let path = dir.path().join(name);
        fs::write(&path, b"a\nb\r\nc\r\nd").unwrap();
        let file = open(dir.path(), name).unwrap();
        let edited = save(&file, "new\na\nb\nc\nd").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new\r\na\nb\r\nc\r\nd");
        save(&edited, "a\nb\nc\nd").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"a\nb\r\nc\r\nd");
    }

    #[test]
    fn concurrent_saves_from_same_revision_do_not_clobber_each_other() {
        let dir = tempdir().unwrap();
        let name = Path::new("source");
        let path = dir.path().join(name);
        fs::write(&path, "old").unwrap();
        let file = open(dir.path(), name).unwrap();
        let (a, b) = thread::scope(|scope| {
            let a = scope.spawn(|| save(&file, "first"));
            let b = scope.spawn(|| save(&file, "second"));
            (a.join().unwrap(), b.join().unwrap())
        });
        assert_ne!(a.is_ok(), b.is_ok());
        let successful = a.or(b).unwrap();
        assert_eq!(successful.text, fs::read_to_string(&path).unwrap());
        save(&file, &successful.text).unwrap(); // Retry an acknowledged write.
        fs::write(&path, "external edit").unwrap();
        assert!(
            save(&successful, "my unsaved draft")
                .unwrap_err()
                .contains("changed on disk")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "external edit");
        fs::remove_file(&path).unwrap();
        assert!(save(&successful, "draft").is_err());
        assert!(!path.exists());
    }

    #[test]
    fn rejects_binary_and_oversize_input_without_touching_the_file() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let name = Path::new("data");
        let path = root.join(name);
        for bytes in [b"\x00valid utf8" as &[u8], b"\x89PNG"] {
            fs::write(&path, bytes).unwrap();
            assert!(open(root, name).unwrap_err().contains("binary"));
        }
        fs::write(&path, "").unwrap();
        let empty = open(root, name).unwrap();
        assert!(save(&empty, "\0oops").is_err());
        assert!(save(&empty, &"x".repeat(MAX_BYTES + 1)).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"");
        File::create(&path)
            .unwrap()
            .set_len(MAX_BYTES as u64 + 1)
            .unwrap();
        assert!(open(root, name).unwrap_err().contains("too large"));
    }

    #[test]
    fn read_only_file_can_be_viewed_and_is_never_replaced() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("readonly");
        fs::write(&path, "original").unwrap();
        let original = path.metadata().unwrap().permissions();
        let mut permissions = original.clone();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        let file = open(dir.path(), Path::new("readonly")).unwrap();
        let result = save(&file, "changed");
        fs::set_permissions(&path, original).unwrap();
        assert!(result.unwrap_err().contains("read-only"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    }

    #[cfg(unix)]
    #[test]
    fn follows_internal_file_aliases_without_following_external_or_git_links() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let root = dir.path().join("project");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), "private").unwrap();
        fs::write(root.join("inside.txt"), "old").unwrap();
        let outside = dir.path().join("outside");
        fs::write(&outside, "outside").unwrap();
        symlink(&outside, root.join("escape")).unwrap();
        symlink(".git/config", root.join("git-alias")).unwrap();
        symlink("inside.txt", root.join("alias")).unwrap();
        symlink(".", root.join("loop")).unwrap();
        let entries = list(&root, Path::new("")).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.path.as_path()).collect::<Vec<_>>(),
            [Path::new("alias"), Path::new("inside.txt")]
        );
        assert!(open(&root, Path::new("escape")).is_err());
        assert!(open(&root, Path::new("git-alias")).is_err());
        let file = open(&root, Path::new("alias")).unwrap();
        let file = save(&file, "new").unwrap();
        assert!(root.join("alias").is_symlink());
        assert_eq!(fs::read_to_string(root.join("inside.txt")).unwrap(), "new");
        fs::remove_file(root.join("alias")).unwrap();
        symlink(&outside, root.join("alias")).unwrap();
        assert!(save(&file, "do not overwrite").is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "outside");
    }
}
