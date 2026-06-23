use std::fs;
use zed_extension_api::{self as zed, Result};

struct RestClientExtension {
    cached_binary_path: Option<String>,
}

impl RestClientExtension {
    // Kept for the production download path; unused on the dev-local branch.
    #[allow(dead_code)]
    fn language_server_binary_path(
        &mut self,
        language_server_id: &zed::LanguageServerId,
    ) -> Result<String> {
        if let Some(path) = &self.cached_binary_path {
            if fs::metadata(path).is_ok_and(|stat| stat.is_file()) {
                return Ok(path.clone());
            }
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let release = zed::latest_github_release(
            "doani/zed-restclient",
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let (platform, arch) = zed::current_platform();
        let asset_name = format!(
            "rest-client-sidecar-{os}-{arch}{extension}",
            os = match platform {
                zed::Os::Mac => "macos",
                zed::Os::Linux => "linux",
                zed::Os::Windows => "windows",
            },
            arch = match arch {
                zed::Architecture::Aarch64 => "aarch64",
                zed::Architecture::X86 => "x86",
                zed::Architecture::X8664 => "x86_64",
            },
            extension = match platform {
                zed::Os::Windows => ".exe",
                _ => "",
            }
        );

        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| format!("no asset found matching {:?}", asset_name))?;

        let version_dir = format!("rest-client-{}", release.version);
        fs::create_dir_all(&version_dir).map_err(|e| format!("failed to create directory: {e}"))?;

        let binary_path = format!("{version_dir}/{asset_name}");

        if !fs::metadata(&binary_path).is_ok_and(|stat| stat.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );

            zed::download_file(
                &asset.download_url,
                &binary_path,
                zed::DownloadedFileType::Uncompressed,
            )
            .map_err(|e| format!("failed to download file: {e}"))?;

            zed::make_file_executable(&binary_path)?;

            let entries =
                fs::read_dir(".").map_err(|e| format!("failed to list working directory {e}"))?;
            for entry in entries {
                let entry = entry.map_err(|e| format!("failed to load directory entry {e}"))?;
                if entry.file_name().to_string_lossy() != version_dir {
                    fs::remove_dir_all(entry.path()).ok();
                }
            }
        }

        self.cached_binary_path = Some(binary_path.clone());
        Ok(binary_path)
    }
}

impl zed::Extension for RestClientExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        _worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        // --- LOCAL DEV (dev-local branch) ---
        // Run the locally-built sidecar directly instead of downloading the
        // upstream prebuilt binary, so local changes (e.g. {{$dotenv}} and
        // showRequest) actually take effect. Run `cargo build -p sidecar
        // --release` first. showRequest is controlled via Zed `lsp` settings;
        // to force it on here instead, add an env entry below, e.g.
        // ("ZED_RESTCLIENT_SHOW_REQUEST".to_string(), "true".to_string()).
        Ok(zed::Command {
            command: "/Users/tammo/repos/zed-restclient/target/release/sidecar".to_string(),
            args: vec![],
            env: vec![],
        })
    }
}

zed::register_extension!(RestClientExtension);

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn test_extension_initialization() {
        let _ext = <RestClientExtension as zed::Extension>::new();
    }
}
