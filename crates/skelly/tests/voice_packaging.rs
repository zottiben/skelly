//! Release assets and installer are exercised in a temporary HOME, never a real install.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn check(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn linux_archive_installs_companion_without_enabling_voice_or_pi() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home with spaces");
    let bin = home.join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    let source = temp.path().join("fixture skelly");
    executable(&source, "#!/bin/sh\necho skelly 99.0.0\n");
    check(
        Command::new("sh")
            .arg(root().join("packaging/linux/bundle.sh"))
            .arg(&source)
            .arg("99.0.0")
            .arg(temp.path())
            .arg("x86_64"),
    );
    let archive = temp.path().join("skelly-v99.0.0-linux-x86_64.tar.gz");
    let unpack = temp.path().join("unpack");
    fs::create_dir(&unpack).unwrap();
    check(
        Command::new("tar")
            .arg("xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&unpack),
    );
    for name in ["index.ts", "package.json", "README.md"] {
        assert_eq!(
            fs::read(unpack.join("share/skelly/pi").join(name)).unwrap(),
            fs::read(root().join("integrations/pi").join(name)).unwrap()
        );
    }
    assert!(!unpack.join("share/skelly/pi/test").exists());
    // A fake network command only serves this test's archive. No API, sudo or real HOME.
    executable(
        &bin.join("curl"),
        r#"#!/bin/sh
case "$2" in
  */skelly-v99.0.0-linux-x86_64.tar.gz) cp "$TEST_ARCHIVE" "$4" ;;
  *) exit 22 ;;
esac
"#,
    );
    executable(
        &bin.join("uname"),
        "#!/bin/sh\ncase \"$1\" in -s) echo Linux;; -m) echo x86_64;; esac\n",
    );
    let path = std::env::join_paths(
        std::iter::once(bin.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    check(
        Command::new("sh")
            .arg(root().join("install.sh"))
            .args(["--version", "v99.0.0", "--force"])
            .env("HOME", &home)
            .env("PATH", path)
            .env("TEST_ARCHIVE", &archive)
            .env_remove("SKELLY_CURRENT_VERSION")
            .env_remove("XDG_DATA_HOME"),
    );
    for name in ["index.ts", "package.json", "README.md"] {
        assert_eq!(
            fs::read(home.join(".local/share/skelly/pi").join(name)).unwrap(),
            fs::read(root().join("integrations/pi").join(name)).unwrap()
        );
    }
    assert!(bin.join("skelly").exists());
    assert!(!home.join(".pi").exists());
    assert!(!home.join(".config/skelly/config.toml").exists());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_bundle_is_signed_with_privacy_metadata_and_companion() {
    let temp = tempfile::tempdir().unwrap();
    // Real signing of a small Mach-O, without opening a window or requesting a mic.
    check(
        Command::new("sh")
            .arg(root().join("packaging/macos/bundle.sh"))
            .args(["/usr/bin/true", "99.0.0"])
            .arg(temp.path()),
    );
    let app = temp.path().join("Skelly.app");
    check(
        Command::new("codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(&app),
    );
    let info = fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
    assert!(info.contains("NSMicrophoneUsageDescription"));
    assert!(info.contains("99.0.0"));
    for name in ["index.ts", "package.json", "README.md"] {
        assert_eq!(
            fs::read(app.join("Contents/Resources/pi").join(name)).unwrap(),
            fs::read(root().join("integrations/pi").join(name)).unwrap()
        );
    }
    let output = Command::new("codesign")
        .args(["--display", "--entitlements", "-"])
        .arg(&app)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("com.apple.security.device.audio-input")
    );
}
