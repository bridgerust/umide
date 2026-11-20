//! One-time, append-only setup of the global guidance read by both agents.
use std::{
    borrow::Cow,
    env, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const BEGIN: &str = "<!-- umide:verification -->";
const END: &str = "<!-- /umide:verification -->";
const GUIDE: &str = "Umide is a desktop coding workspace with local mobile and web previews. Inside Umide, match checks to the request. Run relevant project tests for logic and check documentation without starting a simulator. For mobile or web UI, use the umide tools to exercise each requested interaction on the intended app and target. Consult workspace_context when needed, capture evidence when useful, and report observations and limits.";

pub(crate) fn install() -> Result<(), String> {
    let home = directories::BaseDirs::new();
    let dir = |variable: &str, default: &str| -> Result<PathBuf, String> {
        let path = env::var_os(variable)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.home_dir().join(default)))
            .ok_or("Cannot locate the agent settings directory.")?;
        if !path.is_absolute() {
            return Err(format!("{variable} must be an absolute path."));
        }
        Ok(path)
    };
    let codex = dir("CODEX_HOME", ".codex").and_then(|dir| install_codex(&dir));
    let claude = dir("CLAUDE_CONFIG_DIR", ".claude")
        .and_then(|dir| install_file(&dir.join("CLAUDE.md")));
    match (codex, claude) {
        (Err(a), Err(b)) => Err(format!("{a}\n{b}")),
        (a, b) => a.and(b),
    }
}

fn install_codex(dir: &Path) -> Result<(), String> {
    // Codex reads only the first nonempty global file, preferring the override.
    let alternate = dir.join("AGENTS.override.md");
    match read_regular(&alternate) {
        Ok(bytes) => {
            let text = document(&bytes).map_err(|e| failure(&alternate, e))?;
            if !text.0.trim().is_empty() {
                return install_file(&alternate);
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(failure(&alternate, e)),
    }
    install_file(&dir.join("AGENTS.md"))
}

fn failure(path: &Path, error: io::Error) -> String {
    format!(
        "Could not update agent guidance in {}: {error}",
        path.display()
    )
}

fn read_regular(path: &Path) -> io::Result<Vec<u8>> {
    if !fs::metadata(path)?.is_file() {
        return Err(io::Error::other("Expected a regular file."));
    }
    let mut file = fs::File::open(path)?;
    // Windows enforces byte-range locks during reads. Read from the handle
    // holding the shared lock so an in-progress append finishes first.
    file.lock_shared()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn install_file(path: &Path) -> Result<(), String> {
    fn update(path: &Path) -> io::Result<()> {
        match read_regular(path) {
            // An installed block also works in a read-only dotfiles checkout.
            Ok(bytes) if addition(&bytes).is_ok_and(|s| s.is_empty()) => {
                return Ok(());
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        fs::create_dir_all(path.parent().unwrap())?;
        let mut options = fs::OpenOptions::new();
        options.read(true).append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // Append on the existing inode, including the target of a dotfiles symlink.
        // Recheck under the OS lock for simultaneous first launches.
        let mut file = options.open(path)?;
        file.lock()?;
        let mut current = Vec::new();
        file.read_to_end(&mut current)?;
        let addition = addition(&current)?;
        if !addition.is_empty() {
            file.write_all(&addition)?;
            file.sync_data()?;
        }
        Ok(())
    }
    update(path).map_err(|error| failure(path, error))
}

#[derive(Clone, Copy)]
enum Encoding {
    Bytes,
    Utf16(bool),
    Utf32(bool),
}

fn document(bytes: &[u8]) -> io::Result<(Cow<'_, str>, Encoding)> {
    use Encoding::*;
    let invalid = || io::Error::other("Unsupported agent guidance file encoding.");
    let (encoding, body) =
        if let Some(body) = bytes.strip_prefix(&[0xff, 0xfe, 0, 0]) {
            (Utf32(true), body)
        } else if let Some(body) = bytes.strip_prefix(&[0, 0, 0xfe, 0xff]) {
            (Utf32(false), body)
        } else if let Some(body) = bytes.strip_prefix(&[0xff, 0xfe]) {
            (Utf16(true), body)
        } else if let Some(body) = bytes.strip_prefix(&[0xfe, 0xff]) {
            (Utf16(false), body)
        } else {
            if bytes.contains(&0) {
                return Err(invalid());
            }
            // All text emitted by Umide is ASCII. Existing non-UTF-8 bytes are only
            // decoded for marker detection and are never rewritten.
            return Ok((
                String::from_utf8_lossy(
                    bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes),
                ),
                Bytes,
            ));
        };
    let text = match encoding {
        Utf16(le) if body.len().is_multiple_of(2) => {
            let units = body.as_chunks::<2>().0.iter().map(|b| {
                if le {
                    u16::from_le_bytes(*b)
                } else {
                    u16::from_be_bytes(*b)
                }
            });
            char::decode_utf16(units)
                .collect::<Result<String, _>>()
                .map_err(|_| invalid())?
        }
        Utf32(le) if body.len().is_multiple_of(4) => body
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&block| {
                char::from_u32(if le {
                    u32::from_le_bytes(block)
                } else {
                    u32::from_be_bytes(block)
                })
            })
            .collect::<Option<String>>()
            .ok_or_else(invalid)?,
        _ => return Err(invalid()),
    };
    Ok((Cow::Owned(text), encoding))
}

fn addition(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let (text, encoding) = document(bytes)?;
    let eol = match text.find(['\r', '\n']).map(|i| &text[i..]) {
        Some(s) if s.starts_with("\r\n") => "\r\n",
        Some(s) if s.starts_with('\r') => "\r",
        _ => "\n",
    };
    if text.split(eol).any(|line| line == END) {
        return Ok(vec![]);
    }
    let section = [BEGIN, "## Umide", "", GUIDE, END, ""].join(eol);
    let suffix = if let Some((_, rest)) = text.rsplit_once(BEGIN) {
        // A write interrupted inside our own block can be completed on retry.
        let written = format!("{BEGIN}{rest}");
        if let Some(rest) = section.strip_prefix(&written) {
            rest.to_owned()
        } else {
            return Err(io::Error::other(
                "An unfinished Umide block contains manual changes.",
            ));
        }
    } else if let Some(tail) = text
        .rsplit(eol)
        .next()
        .filter(|s| s.starts_with("<!-- umide:") && BEGIN.starts_with(s))
    {
        section[tail.len()..].to_owned()
    } else {
        let gap = if text.is_empty() || text.ends_with(&format!("{eol}{eol}")) {
            String::new()
        } else if text.ends_with(eol) {
            eol.into()
        } else {
            eol.repeat(2)
        };
        format!("{gap}{section}")
    };
    Ok(match encoding {
        Encoding::Bytes => suffix.into_bytes(),
        Encoding::Utf16(le) => suffix
            .encode_utf16()
            .flat_map(|c| if le { c.to_le_bytes() } else { c.to_be_bytes() })
            .collect(),
        Encoding::Utf32(le) => suffix
            .chars()
            .flat_map(|c| {
                if le {
                    (c as u32).to_le_bytes()
                } else {
                    (c as u32).to_be_bytes()
                }
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn chooses_only_the_nonempty_global_override_and_retries_without_duplicates() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let regular = root.join("AGENTS.md");
        let alternate = root.join("AGENTS.override.md");
        fs::write(&regular, "My settings").unwrap();
        fs::write(&alternate, "Dotfiles\r\n").unwrap();
        std::thread::scope(|s| {
            for _ in 0..6 {
                s.spawn(move || install_codex(root).unwrap());
            }
        });
        let text = fs::read_to_string(&alternate).unwrap();
        assert!(text.starts_with("Dotfiles\r\n\r\n"));
        assert_eq!(text.matches(BEGIN).count(), 1);
        assert!(!text.replace("\r\n", "").contains('\n'));
        assert_eq!(fs::read_to_string(&regular).unwrap(), "My settings");
        fs::write(&alternate, " \n").unwrap();
        install_codex(root).unwrap();
        assert!(
            fs::read_to_string(&regular)
                .unwrap()
                .starts_with("My settings\n\n")
        );
        assert_eq!(fs::read_to_string(&alternate).unwrap(), " \n");
    }

    #[test]
    fn waits_for_the_override_writer_before_choosing_the_global_file() {
        use std::{sync::mpsc, time::Duration};

        let dir = tempdir().unwrap();
        let root = dir.path();
        let alternate = root.join("AGENTS.override.md");
        let mut writer = fs::File::create(&alternate).unwrap();
        writer.lock().unwrap();

        std::thread::scope(|scope| {
            let (started, ready) = mpsc::channel();
            let (done, result) = mpsc::channel();
            scope.spawn(move || {
                started.send(()).unwrap();
                done.send(install_codex(root)).unwrap();
            });
            ready.recv().unwrap();
            let waiting = result.recv_timeout(Duration::from_millis(100));
            writer.write_all(b"Personal override.").unwrap();
            drop(writer);
            assert!(matches!(waiting, Err(mpsc::RecvTimeoutError::Timeout)));
            result
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap();
        });
        assert!(!root.join("AGENTS.md").exists());
        assert!(
            fs::read_to_string(alternate)
                .unwrap()
                .starts_with("Personal override.\n\n")
        );
    }

    #[test]
    fn an_installed_read_only_guidance_file_needs_no_write_access() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        install_file(&path).unwrap();
        let original = fs::read(&path).unwrap();
        let permissions = fs::metadata(&path).unwrap().permissions();
        let mut read_only = permissions.clone();
        read_only.set_readonly(true);
        fs::set_permissions(&path, read_only).unwrap();
        let result = install_file(&path);
        fs::set_permissions(&path, permissions).unwrap();
        result.unwrap();
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    fn preserves_existing_encodings_and_completes_an_interrupted_append() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        let mut utf16 = vec![0xff, 0xfe];
        utf16.extend("Préférences\r\n".encode_utf16().flat_map(u16::to_le_bytes));
        for original in [
            b"\xef\xbb\xbfUser settings".to_vec(),
            b"Pr\xe9f\xe9rences\r".to_vec(),
            utf16,
        ] {
            fs::write(&path, &original).unwrap();
            install_file(&path).unwrap();
            let first = fs::read(&path).unwrap();
            assert!(first.starts_with(&original));
            assert_eq!(document(&first).unwrap().0.matches(BEGIN).count(), 1);
            install_file(&path).unwrap();
            assert_eq!(fs::read(&path).unwrap(), first);
        }
        fs::write(&path, format!("My file\n\n{BEGIN}\n## Um")).unwrap();
        install_file(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("## Umide").count(), 1);
        assert!(text.ends_with(&format!("{END}\n")));
        fs::write(&path, [0, 4, 21]).unwrap();
        assert!(install_file(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), [0, 4, 21]);
    }

    #[cfg(unix)]
    #[test]
    fn updates_dotfile_target_without_replacing_the_link_or_its_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempdir().unwrap();
        let target = dir.path().join("shared-guide.md");
        let link = dir.path().join("CLAUDE.md");
        fs::write(&target, "Personal guidance.").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&target, &link).unwrap();
        install_file(&link).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), target);
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(
            fs::read_to_string(&target)
                .unwrap()
                .starts_with("Personal guidance.\n\n")
        );
    }
}
