//! Zed extension entry point. All it does is tell Zed how to launch csv-ls,
//! resolving the binary in this order:
//!
//! 1. `lsp.csv-ls.binary.path` from Zed settings (explicit user override),
//! 2. a `csv-ls` found on the worktree's PATH (e.g. built via
//!    `pixi run build-lsp` and symlinked somewhere on PATH),
//! 3. a download from this repository's GitHub releases (requires the repo
//!    to be public), cached per version in the extension's work directory.

use zed_extension_api::{self as zed, settings::LspSettings, LanguageServerId, Result};

const GITHUB_REPO: &str = "ickc/zed-csv-toolkit";
const SERVER_BIN: &str = "csv-ls";

struct CsvToolkitExtension {
    cached_binary_path: Option<String>,
}

impl CsvToolkitExtension {
    fn language_server_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<String> {
        if let Ok(settings) = LspSettings::for_worktree(SERVER_BIN, worktree) {
            if let Some(path) = settings.binary.and_then(|binary| binary.path) {
                return Ok(path);
            }
        }

        if let Some(path) = worktree.which(SERVER_BIN) {
            return Ok(path);
        }

        if let Some(path) = &self.cached_binary_path {
            if std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
                return Ok(path.clone());
            }
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let release = zed::latest_github_release(
            GITHUB_REPO,
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let (platform, arch) = zed::current_platform();
        let target = match (platform, arch) {
            (zed::Os::Mac, zed::Architecture::Aarch64) => "aarch64-apple-darwin",
            (zed::Os::Mac, zed::Architecture::X8664) => "x86_64-apple-darwin",
            (zed::Os::Linux, zed::Architecture::X8664) => "x86_64-unknown-linux-gnu",
            (zed::Os::Linux, zed::Architecture::Aarch64) => "aarch64-unknown-linux-gnu",
            (zed::Os::Windows, zed::Architecture::X8664) => "x86_64-pc-windows-msvc",
            (zed::Os::Windows, zed::Architecture::Aarch64) => "aarch64-pc-windows-msvc",
            other => return Err(format!("unsupported platform: {other:?}")),
        };
        let bin_name = match platform {
            zed::Os::Windows => format!("{SERVER_BIN}.exe"),
            _ => SERVER_BIN.to_string(),
        };
        // Version-less asset names keep the release workflow simple; the
        // version is tracked by the cache directory below instead.
        let asset_name = format!("{SERVER_BIN}-{target}.tar.gz");
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| format!("release {} has no asset {asset_name}", release.version))?;

        let version_dir = format!("{SERVER_BIN}-{}", release.version);
        let binary_path = format!("{version_dir}/{bin_name}");

        if !std::fs::metadata(&binary_path).is_ok_and(|m| m.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            zed::download_file(
                &asset.download_url,
                &version_dir,
                zed::DownloadedFileType::GzipTar,
            )?;
            zed::make_file_executable(&binary_path)?;

            // Drop caches of older versions.
            if let Ok(entries) = std::fs::read_dir(".") {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with(SERVER_BIN) && name != version_dir {
                        let _ = std::fs::remove_dir_all(entry.path());
                    }
                }
            }
        }

        // Maintain a versionless alias (work/csv-toolkit/bin/csv-ls) for use
        // from outside Zed — e.g. the markdown-preview task in the README —
        // so user config doesn't have to hardcode the release version.
        // Remove-then-copy avoids ETXTBSY if the old copy is running.
        let stable_path = format!("bin/{bin_name}");
        if std::fs::create_dir_all("bin").is_ok() {
            let _ = std::fs::remove_file(&stable_path);
            if std::fs::copy(&binary_path, &stable_path).is_ok() {
                let _ = zed::make_file_executable(&stable_path);
            }
        }

        self.cached_binary_path = Some(binary_path.clone());
        Ok(binary_path)
    }
}

impl zed::Extension for CsvToolkitExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        Ok(zed::Command {
            command: self.language_server_binary_path(language_server_id, worktree)?,
            args: Vec::new(),
            env: Vec::new(),
        })
    }
}

zed::register_extension!(CsvToolkitExtension);
