use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use semver::Version;
use serde::Deserialize;
use umide_core::{
    directory::Directory,
    meta::{self, ReleaseType},
};

const RELEASES_API: &str = "https://api.github.com/repos/bridgerust/umide/releases";

#[derive(Clone, Deserialize, Debug)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub target_commitish: String,
    pub assets: Vec<ReleaseAsset>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(skip)]
    pub version: String,
}

#[derive(Clone, Deserialize, Debug)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
}

fn parse_version(tag: &str) -> Result<Version, semver::Error> {
    Version::parse(tag.strip_prefix('v').unwrap_or(tag))
}

impl ReleaseInfo {
    fn semver(&self, allow_prereleases: bool) -> Option<Version> {
        let version = parse_version(&self.tag_name).ok()?;
        (!self.draft
            && (allow_prereleases || (!self.prerelease && version.pre.is_empty())))
        .then_some(version)
    }

    fn nightly_version(&self) -> Option<String> {
        if self.draft || self.tag_name != "nightly" {
            return None;
        }
        let commit = self.target_commitish.get(..7)?;
        if !self.target_commitish.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(format!("{}+Nightly.{commit}", env!("CARGO_PKG_VERSION")))
    }

    pub fn is_update(&self) -> bool {
        self.installable_asset().is_some()
            && self.is_update_for(
                meta::RELEASE,
                meta::RELEASE_TAG.unwrap_or(meta::VERSION),
            )
    }

    fn installable_asset(&self) -> Option<&ReleaseAsset> {
        let name = match std::env::consts::OS {
            "macos" => "UMIDE-macos.dmg",
            #[cfg(feature = "portable")]
            "windows" => "UMIDE-windows-portable.zip",
            #[cfg(not(feature = "portable"))]
            "windows" => "UMIDE-windows.msi",
            _ => return None,
        };
        self.assets.iter().find(|asset| asset.name == name)
    }

    fn is_update_for(&self, channel: ReleaseType, installed: &str) -> bool {
        match channel {
            ReleaseType::Debug => false,
            ReleaseType::Nightly => self
                .nightly_version()
                .is_some_and(|version| version != installed),
            ReleaseType::Stable => {
                let Ok(installed) = parse_version(installed) else {
                    return false;
                };
                self.semver(!installed.pre.is_empty())
                    .is_some_and(|version| {
                        version.cmp_precedence(&installed).is_gt()
                    })
            }
        }
    }
}

fn latest_versioned_release(
    releases: impl IntoIterator<Item = ReleaseInfo>,
    allow_prereleases: bool,
) -> Result<ReleaseInfo> {
    let (version, mut release) = releases
        .into_iter()
        .filter_map(|release| Some((release.semver(allow_prereleases)?, release)))
        .max_by(|(a, _), (b, _)| a.cmp_precedence(b))
        .ok_or_else(|| anyhow!("no versioned release for this update channel"))?;
    release.version = version.to_string();
    Ok(release)
}

fn fetch_release_json(path: &str) -> Result<String> {
    let resp = umide_proxy::get_url(format!("{RELEASES_API}{path}"), Some("Umide"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("get release info failed {}", resp.text()?));
    }
    Ok(resp.text()?)
}

pub fn get_latest_release() -> Result<ReleaseInfo> {
    match meta::RELEASE {
        ReleaseType::Debug => Err(anyhow!("no release for debug")),
        ReleaseType::Nightly => {
            let mut release: ReleaseInfo =
                serde_json::from_str(&fetch_release_json("/tags/nightly")?)?;
            release.version = release
                .nightly_version()
                .ok_or_else(|| anyhow!("nightly release has no commit SHA"))?;
            Ok(release)
        }
        ReleaseType::Stable => {
            let installed =
                parse_version(meta::RELEASE_TAG.unwrap_or(meta::VERSION))?;
            if installed.pre.is_empty() {
                let release = serde_json::from_str(&fetch_release_json("/latest")?)?;
                latest_versioned_release([release], false)
            } else {
                let releases: Vec<ReleaseInfo> =
                    serde_json::from_str(&fetch_release_json("?per_page=100")?)?;
                latest_versioned_release(releases, true)
            }
        }
    }
}

pub fn download_release(release: &ReleaseInfo) -> Result<PathBuf> {
    let asset = release
        .installable_asset()
        .ok_or_else(|| anyhow!("no installer for this platform"))?;
    let dir =
        Directory::updates_directory().ok_or_else(|| anyhow!("no directory"))?;
    let file_path = dir.join(&asset.name);
    let mut resp = umide_proxy::get_url(&asset.browser_download_url, None)?;
    if !resp.status().is_success() {
        return Err(anyhow!("download file error {}", resp.text()?));
    }
    let mut out = std::fs::File::create(&file_path)?;
    resp.copy_to(&mut out)?;
    Ok(file_path)
}

#[cfg(target_os = "macos")]
pub fn extract(src: &Path, process_path: &Path) -> Result<PathBuf> {
    let info = dmg::Attach::new(src).with()?;
    let dest = process_path.parent().ok_or_else(|| anyhow!("no parent"))?;
    let dest = if dest.file_name().and_then(|s| s.to_str()) == Some("MacOS") {
        dest.parent().unwrap().parent().unwrap().parent().unwrap()
    } else {
        dest
    };
    std::fs::remove_dir_all(dest.join("UMIDE.app"))?;
    fs_extra::copy_items(
        &[info.mount_point.join("UMIDE.app")],
        dest,
        &fs_extra::dir::CopyOptions {
            overwrite: true,
            skip_exist: false,
            buffer_size: 64000,
            copy_inside: true,
            content_only: false,
            depth: 0,
        },
    )?;
    Ok(dest.join("UMIDE.app"))
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
pub fn extract(src: &Path, process_path: &Path) -> Result<PathBuf> {
    let tar_gz = std::fs::File::open(src)?;
    let tar = flate2::read::GzDecoder::new(tar_gz);
    let mut archive = tar::Archive::new(tar);
    let parent = src.parent().ok_or_else(|| anyhow::anyhow!("no parent"))?;
    archive.unpack(parent)?;
    std::fs::remove_file(process_path)?;
    std::fs::copy(parent.join("UMIDE").join("umide"), process_path)?;
    Ok(process_path.to_path_buf())
}

#[cfg(all(target_os = "windows", feature = "portable"))]
pub fn extract(src: &Path, process_path: &Path) -> Result<PathBuf> {
    let parent = src
        .parent()
        .ok_or_else(|| anyhow::anyhow!("src has no parent"))?;
    let dst_parent = process_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("process_path has no parent"))?;

    {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(src)?)?;
        archive.extract(parent)?;
    }

    // TODO(dbuga): instead of replacing the exe, run the msi installer for non-portable
    // TODO(dbuga): there's a very slight chance the user might end up with a backup file without a working .exe
    std::fs::rename(process_path, dst_parent.join("umide.exe.bak"))?;
    std::fs::copy(parent.join("umide.exe"), process_path)?;

    Ok(process_path.to_path_buf())
}

#[cfg(all(target_os = "windows", not(feature = "portable")))]
pub fn extract(src: &Path, _process_path: &Path) -> Result<PathBuf> {
    // We downloaded an uncompressed msi installer, nothing to extract.
    // On the other hand, we need to run this msi so pass its path back out.
    Ok(src.to_path_buf())
}

#[cfg(target_os = "macos")]
pub fn restart(path: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let _ = std::process::Command::new("open")
        .arg("-n")
        .arg(path)
        .arg("--args")
        .arg("-n")
        .exec();
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd"))]
pub fn restart(path: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let _ = std::process::Command::new(path).arg("-n").exec();
    Ok(())
}

#[cfg(all(target_os = "windows", feature = "portable"))]
pub fn restart(path: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x00000008;
    let process_id = std::process::id();
    let path = path
        .to_str()
        .ok_or_else(|| anyhow!("can't get path to str"))?;
    std::process::Command::new("cmd")
        .raw_arg(format!(
            r#"/C taskkill /PID {process_id} & start "" "{path}""#
        ))
        .creation_flags(DETACHED_PROCESS)
        .spawn()?;
    Ok(())
}

#[cfg(all(target_os = "windows", not(feature = "portable")))]
pub fn restart(path: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x00000008;
    let process_id = std::process::id();
    let path = path
        .to_str()
        .ok_or_else(|| anyhow!("can't get path to str"))?;

    let lapce_exe = std::env::current_exe()
        .map_err(|err| anyhow!("can't get path to exe").context(err))?;
    let lapce_exe = lapce_exe
        .to_str()
        .ok_or_else(|| anyhow!("can't convert exe path to str"))?;

    std::process::Command::new("cmd")
        .raw_arg(format!(
            r#"/C taskkill /PID {process_id} & msiexec /i "{path}" /qb & start "" "{lapce_exe}""#,
        ))
        .creation_flags(DETACHED_PROCESS)
        .spawn()?;
    Ok(())
}

#[cfg(all(target_os = "windows", feature = "portable"))]
pub fn cleanup() {
    // Clean up backup exe after an update
    if let Ok(process_path) = std::env::current_exe() {
        if let Some(dst_parent) = process_path.parent() {
            if let Err(err) = std::fs::remove_file(dst_parent.join("umide.exe.bak"))
            {
                tracing::error!("{:?}", err);
            }
        }
    }
}

#[cfg(any(
    not(target_os = "windows"),
    all(target_os = "windows", not(feature = "portable"))
))]
pub fn cleanup() {
    // Nothing to do yet
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag_name: &str, prerelease: bool) -> ReleaseInfo {
        ReleaseInfo {
            tag_name: tag_name.into(),
            target_commitish: "abcdef0123456789".into(),
            assets: Vec::new(),
            draft: false,
            prerelease,
            version: String::new(),
        }
    }

    #[test]
    fn beta_never_offers_an_older_or_equal_version() {
        let installed = "v0.4.0-beta.1";
        for (tag, prerelease) in [
            ("v0.3.3", false),
            ("v0.4.0-beta.1", true),
            ("v0.4.0-beta.1+rebuilt", true),
        ] {
            assert!(
                !release(tag, prerelease)
                    .is_update_for(ReleaseType::Stable, installed)
            );
        }
        assert!(
            release("v0.4.0-beta.2", true)
                .is_update_for(ReleaseType::Stable, installed)
        );
        assert!(
            release("v0.4.0", false).is_update_for(ReleaseType::Stable, installed)
        );
    }

    #[test]
    fn stable_ignores_prereleases_and_debug_never_updates() {
        let installed = "0.3.3";
        assert!(
            !release("v0.4.0-beta.1", true)
                .is_update_for(ReleaseType::Stable, installed)
        );
        assert!(
            !release("v0.4.0-beta.1", false)
                .is_update_for(ReleaseType::Stable, installed)
        );
        assert!(
            !release("v0.4.0", true).is_update_for(ReleaseType::Stable, installed)
        );
        assert!(
            release("v0.4.0", false).is_update_for(ReleaseType::Stable, installed)
        );
        assert!(
            !release("v0.4.0", false).is_update_for(ReleaseType::Debug, installed)
        );
    }

    #[test]
    fn selects_by_semver_and_ignores_drafts_and_non_versioned_tags() {
        let mut draft = release("v1.0.0", false);
        draft.draft = true;
        let releases = [
            draft,
            release("v0.4.0-beta.10", true),
            release("v0.4.0-beta.9", true),
            release("v0.3.3", false),
            release("nightly", false),
        ];
        assert_eq!(
            latest_versioned_release(releases.clone(), true)
                .unwrap()
                .version,
            "0.4.0-beta.10"
        );
        assert_eq!(
            latest_versioned_release(releases, false).unwrap().version,
            "0.3.3"
        );
    }

    #[test]
    fn nightly_requires_a_different_commit_and_ignores_other_releases() {
        let installed = format!("{}+Nightly.abcdef0", env!("CARGO_PKG_VERSION"));
        let mut nightly = release("nightly", false);
        assert!(!nightly.is_update_for(ReleaseType::Nightly, &installed));

        nightly.target_commitish = "0123456789abcdef".into();
        assert!(nightly.is_update_for(ReleaseType::Nightly, &installed));

        nightly.target_commitish = "main".into();
        assert!(!nightly.is_update_for(ReleaseType::Nightly, &installed));
        assert!(
            !release("v1.0.0", false)
                .is_update_for(ReleaseType::Nightly, &installed)
        );
    }
}
