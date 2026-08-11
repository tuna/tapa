//! Self-update (`tapa update`) and the automatic release check.
//!
//! # Automatic check design
//!
//! The check is asynchronous, cached, and never blocks or fails a run:
//!
//! - At the end of every invocation ([`finish`]) the cached result is
//!   read; if it names a release newer than this binary, a warning is
//!   printed as the last line of output.
//! - When the cache is older than [`CHECK_INTERVAL_SECS`], [`finish`]
//!   stamps it and spawns a detached `tapa update-check` child that
//!   fetches the latest release tag and rewrites the cache. The parent
//!   never waits for the child, so a check failure (offline, rate
//!   limited, DNS) is invisible until the next stamp elapses.
//! - The child is a full `tapa` invocation, but `main` skips remote
//!   bootstrap and the update check itself for it, so it cannot
//!   trigger an SSH sync or recursively spawn checkers.
//!
//! Setting `TAPA_NO_UPDATE_CHECK` disables all of the above.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::steps::version::VERSION;

/// How long a cached check result is trusted before re-checking.
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// GitHub API endpoint naming the latest (non-prerelease, non-draft)
/// release — the same notion of "latest" that `install.sh` downloads.
const RELEASES_API_URL: &str = "https://api.github.com/repos/tuna/tapa/releases/latest";

/// Asset URL of the latest release tarball (redirects to the CDN).
const LATEST_TARBALL_URL: &str =
    "https://github.com/tuna/tapa/releases/latest/download/tapa.tar.gz";

/// Bound on the background API fetch; the explicit `tapa update`
/// download is unbounded (large asset, user can interrupt).
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Set to disable the automatic release check entirely.
pub const NO_UPDATE_CHECK_ENV: &str = "TAPA_NO_UPDATE_CHECK";

/// Arguments of `tapa update` (`tapa upgrade`).
#[derive(Debug, Parser)]
#[command(name = "update", about = "Update TAPA to the latest release.")]
pub struct UpdateArgs {}

/// Arguments of the hidden `tapa update-check` background worker.
#[derive(Debug, Parser)]
#[command(name = "update-check", hide = true)]
pub struct UpdateCheckArgs {}

/// On-disk record of the last release check. `latest_tag` survives a
/// failed re-check: stamping the cache keeps the old tag so a later
/// warning can still use it.
#[derive(Debug, Default, Serialize, Deserialize)]
struct UpdateCache {
    checked_at_unix: u64,
    #[serde(default)]
    latest_tag: Option<String>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Cache file path under the user cache root (`$XDG_CACHE_HOME` or
/// `$HOME/.cache`), or `None` when no home is known — in which case
/// the whole feature inertly switches off.
fn cache_file_path() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()
        .map(|d| d.cache_dir().to_path_buf())
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("tapa").join("update-check.json"))
}

/// A corrupt or unreadable cache is not an error: it means "no check
/// result yet", i.e. stale and with nothing to warn about.
fn read_cache(path: &Path) -> Option<UpdateCache> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_cache(path: &Path, cache: &UpdateCache) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string(cache)?;
    std::fs::write(path, body)
}

fn is_stale(cache: Option<&UpdateCache>, now_unix_ts: u64) -> bool {
    cache.is_none_or(|c| now_unix_ts.saturating_sub(c.checked_at_unix) >= CHECK_INTERVAL_SECS)
}

/// A dot-separated numeric version (`0.1.20260811[.patch]`).
///
/// TAPA versions are date-based and carry an optional fourth patch
/// segment, which semver cannot represent, so comparison is numeric
/// component by component — `Vec` ordering is lexicographic with a
/// shorter prefix ordering first, exactly the required semantics.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReleaseVersion(Vec<u64>);

impl ReleaseVersion {
    fn parse(s: &str) -> Option<Self> {
        let components: Option<Vec<u64>> = s.split('.').map(|c| c.parse().ok()).collect();
        let components = components?;
        if components.is_empty() {
            return None;
        }
        Some(Self(components))
    }
}

/// Parse a release tag (`v0.1.20260811`); unparseable tags are treated
/// as "nothing known" rather than an error.
fn release_version(tag: &str) -> Option<ReleaseVersion> {
    ReleaseVersion::parse(tag.strip_prefix('v').unwrap_or(tag))
}

/// The version of this binary. `VERSION` is a compile-time constant in
/// the `major.minor.date[.patch]` format, so a parse failure is a bug.
fn current_version() -> ReleaseVersion {
    ReleaseVersion::parse(VERSION).expect("VERSION is a numeric version")
}

/// Whether `latest_tag` names a release strictly newer than this
/// binary.
fn is_newer_release(latest_tag: &str) -> bool {
    release_version(latest_tag).is_some_and(|latest| latest > current_version())
}

/// End-of-run hook: print a cached update warning, then kick off a
/// background re-check when the cache is stale. Every failure mode is
/// swallowed by design — this must never change a run's outcome.
pub fn finish() {
    if std::env::var_os(NO_UPDATE_CHECK_ENV).is_some() {
        return;
    }
    warn_if_outdated();
    spawn_check_if_stale();
}

/// Print the cached "new release available" notice, colored when
/// stderr is a terminal.
fn warn_if_outdated() {
    let Some(path) = cache_file_path() else {
        return;
    };
    let Some(tag) = read_cache(&path).and_then(|c| c.latest_tag) else {
        return;
    };
    if !is_newer_release(&tag) {
        return;
    }
    // Piped stdout is block-buffered; flush so the warning really is
    // the last line of output rather than racing ahead of it.
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let message = format!(
        "a new TAPA release is available: {tag} (installed: {VERSION}) \
         — run `tapa update` to upgrade"
    );
    if std::io::stderr().is_terminal() {
        let style = anstyle::Style::new()
            .fg_color(Some(anstyle::AnsiColor::Yellow.into()))
            .bold();
        eprintln!("{style}tapa: warning:{style:#} {message}");
    } else {
        eprintln!("tapa: warning: {message}");
    }
}

/// Stamp the cache and spawn the detached `update-check` worker. The
/// stamp comes first so a burst of invocations spawns one checker and
/// a failed check is not retried until the interval elapses.
fn spawn_check_if_stale() {
    let Some(path) = cache_file_path() else {
        return;
    };
    let now = now_unix();
    let cache = read_cache(&path);
    if !is_stale(cache.as_ref(), now) {
        return;
    }
    let stamped = UpdateCache {
        checked_at_unix: now,
        latest_tag: cache.and_then(|c| c.latest_tag),
    };
    if write_cache(&path, &stamped).is_err() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // Give the child a work dir inside the cache so it does not create
    // `./work.out` in the user's cwd as a side effect of `main`.
    let work_dir = path.with_file_name("update-check-work");
    let _ = Command::new(exe)
        .arg("--work-dir")
        .arg(work_dir)
        .arg("update-check")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Body of the hidden `tapa update-check` worker: fetch the latest tag
/// and record it. Best-effort — a failure leaves the parent's stamp in
/// place, which defers the next attempt by one interval.
pub fn run_update_check() {
    if std::env::var_os(NO_UPDATE_CHECK_ENV).is_some() {
        return;
    }
    let Some(path) = cache_file_path() else {
        return;
    };
    match fetch_latest_tag() {
        Ok(tag) => {
            let cache = UpdateCache {
                checked_at_unix: now_unix(),
                latest_tag: Some(tag),
            };
            if let Err(e) = write_cache(&path, &cache) {
                log::debug!("update-check: cannot write cache: {e}");
            }
        }
        Err(e) => log::debug!("update-check: fetch failed: {e}"),
    }
}

/// Fetch the latest release tag from the GitHub API.
fn fetch_latest_tag() -> std::result::Result<String, String> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(CHECK_TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(RELEASES_API_URL)
        .header("User-Agent", concat!("tapa/", env!("CARGO_PKG_NAME")))
        .call()
        .map_err(|e| e.to_string())?;
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let release: Release = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    Ok(release.tag_name)
}

/// Download the latest release tarball to `dest`.
fn download_tarball(dest: &Path) -> std::result::Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().build().into();
    let mut response = agent
        .get(LATEST_TARBALL_URL)
        .header("User-Agent", concat!("tapa/", env!("CARGO_PKG_NAME")))
        .call()
        .map_err(|e| e.to_string())?;
    let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    std::io::copy(&mut response.body_mut().as_reader(), &mut file).map_err(|e| e.to_string())?;
    Ok(())
}

/// The installation root of this binary in the release layout
/// (`<root>/usr/bin/tapa`, matching `install.sh`). `current_exe`
/// resolves symlinks (`/proc/self/exe`), so a `/usr/local/bin/tapa`
/// symlink correctly yields `/opt/tapa`.
fn detect_install_root() -> Result<PathBuf> {
    let exe = std::env::current_exe().and_then(|p| p.canonicalize())?;
    let root = exe
        .parent()
        .filter(|bin| bin.file_name() == Some("bin".as_ref()))
        .and_then(Path::parent)
        .filter(|usr| usr.file_name() == Some("usr".as_ref()))
        .and_then(Path::parent);
    match root {
        Some(root) if root.join("usr").join("bin").is_dir() => Ok(root.to_path_buf()),
        _ => Err(CliError::Update(format!(
            "this tapa binary (`{}`) is not part of a release installation; \
             reinstall with install.sh: \
             https://github.com/tuna/tapa/blob/main/install.sh",
            exe.display(),
        ))),
    }
}

/// `tapa update` — download the latest release and replace the current
/// installation in place.
///
/// Symlinks and `PATH` edits made by the original install point into
/// the install root, so replacing its contents leaves them valid;
/// unlike the background check, failures here are real errors because
/// the user explicitly asked.
pub fn run_update(_args: &UpdateArgs, _ctx: &CliContext) -> Result<()> {
    let root = detect_install_root()?;
    println!("Checking for the latest TAPA release...");
    let tag = fetch_latest_tag()
        .map_err(|e| CliError::Update(format!("cannot query the latest release: {e}")))?;
    if release_version(&tag).is_none() {
        return Err(CliError::Update(format!(
            "unexpected release tag format: `{tag}`"
        )));
    }
    if !is_newer_release(&tag) {
        println!("TAPA is already up to date ({VERSION}).");
        if let Some(path) = cache_file_path() {
            let cache = UpdateCache {
                checked_at_unix: now_unix(),
                latest_tag: Some(tag),
            };
            let _ = write_cache(&path, &cache);
        }
        return Ok(());
    }

    println!("Downloading TAPA {tag} from: {LATEST_TARBALL_URL}...");
    let download_dir = tempfile::tempdir()?;
    let tarball = download_dir.path().join("tapa.tar.gz");
    download_tarball(&tarball).map_err(|e| CliError::Update(format!("download failed: {e}")))?;

    println!("Installing TAPA {tag} to \"{}\"...", root.display());
    std::fs::remove_dir_all(&root).map_err(|e| {
        CliError::Update(format!(
            "cannot replace `{}`: {e} — retry with elevated privileges \
             (e.g. `sudo tapa update`) if it is not writable",
            root.display(),
        ))
    })?;
    std::fs::create_dir_all(&root)?;
    let file = std::fs::File::open(&tarball)?;
    tar::Archive::new(flate2::read::GzDecoder::new(file)).unpack(&root)?;

    if let Some(path) = cache_file_path() {
        let cache = UpdateCache {
            checked_at_unix: now_unix(),
            latest_tag: Some(tag.clone()),
        };
        let _ = write_cache(&path, &cache);
    }
    println!("Updated TAPA to {tag} in \"{}\".", root.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_tags_parse_as_date_based_versions() {
        // The check hinges on TAPA's `major.minor.date[.patch]` tags
        // comparing correctly; the patch segment makes them invalid
        // semver, which is why the comparator is numeric.
        let older = release_version("v0.1.20250101").expect("parses");
        assert!(older < current_version());
        assert!(release_version("v0.1.20250101.1").expect("parses") > older);
        assert_eq!(
            release_version("v0.1.20250101"),
            release_version("0.1.20250101"),
        );
        assert!(release_version("not-a-version").is_none());
    }

    #[test]
    fn newer_release_only_when_strictly_newer() {
        assert!(is_newer_release("v9999.1.1"));
        assert!(!is_newer_release(VERSION));
        assert!(!is_newer_release("v0.0.1"));
        assert!(!is_newer_release("garbage"));
    }

    #[test]
    fn cache_is_stale_when_missing_or_old() {
        let fresh = UpdateCache {
            checked_at_unix: 1_000,
            latest_tag: None,
        };
        assert!(!is_stale(Some(&fresh), 1_000 + CHECK_INTERVAL_SECS - 1));
        assert!(is_stale(Some(&fresh), 1_000 + CHECK_INTERVAL_SECS));
        assert!(is_stale(None, 1_000));
    }

    #[test]
    fn corrupt_cache_reads_as_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("update-check.json");
        std::fs::write(&path, "{ not json").expect("write");
        assert!(read_cache(&path).is_none());
        std::fs::write(&path, r#"{"checked_at_unix": 7}"#).expect("write");
        let cache = read_cache(&path).expect("parses");
        assert_eq!(cache.checked_at_unix, 7);
        assert!(cache.latest_tag.is_none());
    }
}
