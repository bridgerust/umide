//! Exercise the real setup commands in a subprocess with a private executable
//! search path. No provider CLI, account, installer or network is required.
#![cfg(unix)]

use super::*;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

const FETCH: &str = r#"#!/bin/sh
set -eu
[ "$#" -eq 11 ] && [ "$1 $2 $3 $4 $5 $6 $7 $8 $9" = "--fail --silent --show-error --location --proto =https --proto-redir =https --output" ] || exit 61
shift 9
case "$2" in
  https://chatgpt.com/codex/install.sh) provider=codex ;;
  https://claude.ai/install.sh) provider=claude ;;
  *) exit 62 ;;
esac
printf '%s\n' "$provider" >> "$UMIDE_SETUP_FIXTURE/downloads"
[ ! -f "$UMIDE_SETUP_FIXTURE/fail-download" ] || exit 63
printf 'provider=%s\n' "$provider" > "$1"
/bin/cat "$UMIDE_SETUP_FIXTURE/installer" >> "$1"
"#;

const INSTALLER: &str = r#"
set -eu
case "$provider" in
  codex) [ "${CODEX_NON_INTERACTIVE:-}" = 1 ] || exit 64 ;;
  claude) [ -n "${BASH_VERSION:-}" ] || exit 65 ;;
esac
[ ! -f "$UMIDE_SETUP_FIXTURE/fail-install" ] || exit 66
/bin/cp "$UMIDE_SETUP_FIXTURE/cli" "$UMIDE_SETUP_FIXTURE/bin/$provider"
/bin/chmod 700 "$UMIDE_SETUP_FIXTURE/bin/$provider"
"#;

const CLI: &str = r#"#!/bin/sh
set -eu
provider=${0##*/}
printf '%s %s\n' "$provider" "$*" >> "$UMIDE_SETUP_FIXTURE/commands"
case "$provider:$*" in
  codex:--version|claude:--version)
    printf '%s fixture-version\nextra banner line\n' "$provider" ;;
  'codex:login status'|'claude:auth status')
    printf 'private account details from a mock status command\n'
    [ -f "$UMIDE_SETUP_FIXTURE/$provider.signed-in" ] ;;
  codex:login|'claude:auth login')
    if [ -f "$UMIDE_SETUP_FIXTURE/hold" ]; then
      printf '%s\n' "$$" > "$UMIDE_SETUP_FIXTURE/held-pid"
      exec /bin/sleep 10
    fi
    [ ! -f "$UMIDE_SETUP_FIXTURE/fail-login" ] || exit 67
    : > "$UMIDE_SETUP_FIXTURE/$provider.signed-in" ;;
  *) exit 68 ;;
esac
"#;

fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "Timed out waiting for mock CLI");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn installs_probes_and_signs_in_with_only_fixture_tools() {
    let Some(root) = std::env::var_os("UMIDE_SETUP_FIXTURE") else {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(bin.join("curl"), FETCH).unwrap();
        fs::set_permissions(bin.join("curl"), fs::Permissions::from_mode(0o700))
            .unwrap();
        fs::write(root.join("installer"), INSTALLER).unwrap();
        fs::write(root.join("cli"), CLI).unwrap();
        // The overrides also suppress fallback to CLIs in /usr/local/bin.
        let run = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "provider_setup::tests::installs_probes_and_signs_in_with_only_fixture_tools", "--nocapture"])
            .env_clear()
            .env("PATH", &bin)
            .env("TMPDIR", &root)
            .env("CODEX_HOME", root.join("codex-config"))
            .env("CLAUDE_CONFIG_DIR", root.join("claude-config"))
            .env("UMIDE_SETUP_FIXTURE", &root)
            .env("UMIDE_CODEX_PATH", bin.join("codex"))
            .env("UMIDE_CLAUDE_PATH", bin.join("claude"))
            .env("UMIDE_WORKBENCH_DATA_DIR", root.join("data"))
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        return;
    };

    let root = Path::new(&root);
    let cancel = AtomicBool::new(false);

    assert!(first_run());
    remember().unwrap();
    assert!(!first_run());
    assert!(root.join("codex-config/AGENTS.md").is_file());
    assert!(root.join("claude-config/CLAUDE.md").is_file());
    for (provider, name) in
        [(Provider::Codex, "codex"), (Provider::Claude, "claude")]
    {
        assert!(!root.join("bin").join(name).exists());
        install(provider, &cancel).unwrap();
        let version = format!("{name} fixture-version");
        assert_eq!(
            probe(provider, &cancel).unwrap(),
            Stage::SignedOut(version.clone())
        );
        sign_in(provider, &cancel).unwrap();
        assert_eq!(probe(provider, &cancel).unwrap(), Stage::Ready(version));
    }
    assert_eq!(
        fs::read_to_string(root.join("downloads")).unwrap(),
        "codex\nclaude\n"
    );
    let commands = fs::read_to_string(root.join("commands")).unwrap();
    assert_eq!(commands.lines().filter(|s| *s == "codex login").count(), 1);
    assert_eq!(
        commands
            .lines()
            .filter(|s| *s == "claude auth login")
            .count(),
        1
    );

    for (marker, message) in [
        ("fail-download", "download"),
        ("fail-install", "installer did not finish"),
    ] {
        fs::write(root.join(marker), "").unwrap();
        assert!(
            install(Provider::Codex, &cancel)
                .unwrap_err()
                .contains(message)
        );
        fs::remove_file(root.join(marker)).unwrap();
    }
    fs::write(root.join("fail-login"), "").unwrap();
    assert!(sign_in(Provider::Claude, &cancel).is_err());
    fs::remove_file(root.join("fail-login")).unwrap();

    // Keep the sign-in process running while other actions target the same
    // provider, then cancel it and verify the recorded process is gone.
    fs::write(root.join("hold"), "").unwrap();
    floem::reactive::Runtime::init_on_ui_thread();
    let setup = ProviderSetup::new();
    setup.sign_in(Provider::Codex);
    let index = ProviderSetup::index(Provider::Codex);
    let original = setup.jobs[index].get_untracked().unwrap();
    setup.sign_in(Provider::Codex);
    setup.refresh(Provider::Codex);
    setup.install(Provider::Codex);
    assert!(Arc::ptr_eq(
        &original,
        &setup.jobs[index].get_untracked().unwrap()
    ));
    assert_eq!(setup.stage(Provider::Codex), Stage::SigningIn);
    let held = root.join("held-pid");
    wait_for(|| {
        fs::read_to_string(&held)
            .ok()
            .is_some_and(|s| !s.trim().is_empty())
    });
    let pid: i32 = fs::read_to_string(held).unwrap().trim().parse().unwrap();
    setup.cancel(Provider::Codex);
    assert!(original.load(Ordering::Acquire));
    wait_for(|| unsafe { libc::kill(pid, 0) } != 0);
    let commands = fs::read_to_string(root.join("commands")).unwrap();
    assert_eq!(commands.lines().filter(|s| *s == "codex login").count(), 2);
}
