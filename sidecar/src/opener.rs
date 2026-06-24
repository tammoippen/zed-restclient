//! Opens the response file in the running Zed instance.
//!
//! Zed's `window/showDocument` LSP request is currently a no-op for local
//! `file://` URIs, so we cannot rely on it to display the response. Instead we
//! locate the Zed **CLI** and hand it the file with `--add`, which reuses the
//! already-open workspace/window (and focuses the existing tab when the response
//! file is already open) rather than spawning a second window or falling back to
//! the OS file association.
//!
//! Note: neither the Zed CLI nor the LSP protocol can target a specific pane or
//! force a split, so the response opens in the active pane of the current
//! window. Move it to a split once and subsequent sends reuse that tab in place.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Environment variable that, when set to an absolute path, overrides Zed CLI
/// discovery (useful for unusual installs or when running a local build).
const ZED_BIN_ENV: &str = "ZED_RESTCLIENT_ZED_BIN";

/// Opens `file_path` in the running Zed window by reusing the current workspace.
/// Returns the CLI that was launched on success, or `None` when no Zed CLI could
/// be found or spawned.
pub fn open_with_zed_cli(file_path: &Path) -> Option<PathBuf> {
    let cli = find_zed_cli()?;
    Command::new(&cli)
        .arg("--add") // add to the currently open workspace, don't open a new window
        .arg(file_path)
        .spawn()
        .ok()
        .map(|_| cli)
}

/// Last-resort: hand the file to the OS so the associated application opens it.
pub fn open_with_os(file_path: &Path) {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(file_path).spawn();
    #[cfg(target_os = "linux")]
    let _ = Command::new("xdg-open").arg(file_path).spawn();
    #[cfg(target_os = "windows")]
    {
        let path_str = file_path.to_string_lossy().into_owned();
        let _ = Command::new("cmd").args(["/C", "start", &path_str]).spawn();
    }
}

/// Locates the Zed CLI binary. Priority:
/// 1. the `ZED_RESTCLIENT_ZED_BIN` override,
/// 2. the *running* Zed instance, derived from this process's ancestry,
/// 3. well-known command names on `PATH`,
/// 4. well-known absolute install locations.
fn find_zed_cli() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(ZED_BIN_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Some(cli) = running_zed_cli() {
        return Some(cli);
    }

    for name in ["zed", "zed-preview", "zeditor", "zed-nightly"] {
        if let Some(cli) = which(name) {
            return Some(cli);
        }
    }

    well_known_zed_paths().into_iter().find(|p| p.is_file())
}

/// Best-effort: find the CLI of the Zed instance that launched this sidecar by
/// walking up the parent-process chain. Returns the sibling `cli` binary inside
/// the macOS app bundle when present, otherwise the discovered Zed executable.
#[cfg(unix)]
fn running_zed_cli() -> Option<PathBuf> {
    let mut pid = std::process::id();
    // Bounded walk up the ancestry; Zed is normally the direct parent.
    for _ in 0..8 {
        let ppid = parent_pid(pid)?;
        if ppid == 0 || ppid == pid {
            break;
        }
        if let Some(exe) = process_exe(ppid)
            && let Some(cli) = zed_cli_from_exe(&exe)
        {
            return Some(cli);
        }
        pid = ppid;
    }
    None
}

#[cfg(not(unix))]
fn running_zed_cli() -> Option<PathBuf> {
    None
}

/// Given an ancestor process's executable path, return the Zed CLI to use if the
/// path looks like a Zed binary; otherwise `None`.
fn zed_cli_from_exe(exe: &Path) -> Option<PathBuf> {
    let name = exe.file_name()?.to_string_lossy().to_ascii_lowercase();

    // A Zed app binary (e.g. `.../Contents/MacOS/zed`): prefer the sibling `cli`.
    if matches!(name.as_str(), "zed" | "zed-preview" | "zed-nightly") {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("cli");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
        // Single-binary install (e.g. Linux): the binary itself opens files.
        if exe.is_file() {
            return Some(exe.to_path_buf());
        }
    }

    // Already a CLI binary sitting next to a `zed` binary (macOS bundle).
    if name == "cli"
        && let Some(dir) = exe.parent()
        && dir.join("zed").is_file()
        && exe.is_file()
    {
        return Some(exe.to_path_buf());
    }

    None
}

/// Minimal `which`: find `name` as an executable file on `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(unix)]
fn parent_pid(pid: u32) -> Option<u32> {
    let out = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(unix)]
fn process_exe(pid: u32) -> Option<PathBuf> {
    let out = Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// Well-known absolute install locations for the Zed CLI, tried in order.
fn well_known_zed_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "macos")]
    {
        for app in ["Zed.app", "Zed Preview.app", "Zed Nightly.app"] {
            paths.push(PathBuf::from(format!(
                "/Applications/{app}/Contents/MacOS/cli"
            )));
        }
        for p in [
            "/opt/homebrew/bin/zed",
            "/opt/homebrew/bin/zed-preview",
            "/usr/local/bin/zed",
            "/usr/local/bin/zed-preview",
        ] {
            paths.push(PathBuf::from(p));
        }
    }

    #[cfg(target_os = "linux")]
    {
        for p in [
            "/usr/bin/zed",
            "/usr/local/bin/zed",
            "/var/lib/flatpak/exports/bin/dev.zed.Zed",
        ] {
            paths.push(PathBuf::from(p));
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        paths.push(home.join(".local/bin/zed"));
        paths.push(home.join(".local/bin/zed-preview"));
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"").unwrap();
    }

    #[test]
    fn zed_cli_from_exe_prefers_sibling_cli() {
        let dir = std::env::temp_dir().join(format!("opener_bundle_{}", uuid::Uuid::new_v4()));
        let macos = dir.join("Contents/MacOS");
        let zed = macos.join("zed");
        let cli = macos.join("cli");
        touch(&zed);
        touch(&cli);

        assert_eq!(zed_cli_from_exe(&zed), Some(cli.clone()));
        // A `cli` next to a `zed` is recognized directly.
        assert_eq!(zed_cli_from_exe(&cli), Some(cli));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn zed_cli_from_exe_single_binary() {
        let dir = std::env::temp_dir().join(format!("opener_single_{}", uuid::Uuid::new_v4()));
        let zed = dir.join("zed");
        touch(&zed);

        // No sibling `cli`: fall back to the binary itself.
        assert_eq!(zed_cli_from_exe(&zed), Some(zed));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn zed_cli_from_exe_ignores_non_zed() {
        assert_eq!(zed_cli_from_exe(Path::new("/usr/bin/bash")), None);
        // A lone `cli` with no sibling `zed` is not assumed to be Zed.
        let dir = std::env::temp_dir().join(format!("opener_lonecli_{}", uuid::Uuid::new_v4()));
        let cli = dir.join("cli");
        touch(&cli);
        assert_eq!(zed_cli_from_exe(&cli), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_zed_cli_honors_env_override() {
        let dir = std::env::temp_dir().join(format!("opener_env_{}", uuid::Uuid::new_v4()));
        let fake = dir.join("my-zed");
        touch(&fake);

        // SAFETY: single-threaded test; set then read one variable.
        unsafe {
            std::env::set_var(ZED_BIN_ENV, &fake);
        }
        assert_eq!(find_zed_cli(), Some(fake));
        unsafe {
            std::env::remove_var(ZED_BIN_ENV);
        }

        fs::remove_dir_all(&dir).ok();
    }
}
