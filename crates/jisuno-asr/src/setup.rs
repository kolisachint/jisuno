//! First-run setup: resolve a model name, report what's installed, and
//! download pre-exported model files from HuggingFace.
//!
//! Pre-exported int8 ONNX files are published on HuggingFace, so no NeMo /
//! Python toolchain is needed — `jisuno setup` fetches them into the per-user
//! data directory (`JISUNO_MODELS_DIR` overrides it, mainly for tests).
//!
//! Downloads are robust:
//!
//! * every file carries a pinned sha256 and byte size; a mismatch deletes the
//!   partial and falls through to the next mirror,
//! * files stream through `<name>.part` and resume with HTTP Range when the
//!   server honors it,
//! * all files land in `<root>/.<id>.staging/` first and the directory is
//!   renamed into place only once everything verifies, so an interrupted
//!   install never yields a half model,
//! * a `<root>/.<id>.lock` file (pid, stale when the pid is dead) keeps the
//!   app and the CLI from downloading the same model concurrently.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{anyhow, Context};

use crate::download::{self, Progress};
use crate::tls::TlsOptions;

/// Where one-time mirrored copies of the model files live, for networks that
/// can't reach huggingface.co (e.g. behind a corporate proxy). Uploaded
/// manually/once — not kept in sync with upstream. See
/// `.github/workflows/mirror-models.yml`.
const MIRROR_BASE: &str = "https://github.com/kolisachint/jisuno/releases/download/models-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    /// Recommended — 25 European languages. PalatineVision/parakeet-tdt-0.6b-v3-onnx
    ParakeetV3Int8,
    /// English-only. istupakov/parakeet-tdt-0.6b-v2-onnx
    ParakeetV2Int8,
}

/// One file of a model: where to fetch it, what it must hash to, and how big
/// it must be. Hashes are the HuggingFace LFS oids (sha256) for the `.onnx`
/// files and `shasum -a 256` of the resolved `vocab.txt` for the text files;
/// all were cross-checked against a local install before pinning.
pub struct FileSpec {
    /// Full HuggingFace resolve URL (primary source).
    pub hf_url: &'static str,
    /// Filename of the same content as a release asset under [`MIRROR_BASE`].
    pub mirror_name: &'static str,
    /// Filename installed under the model dir.
    pub local_name: &'static str,
    /// Expected lowercase hex sha256 of the file.
    pub sha256: &'static str,
    /// Expected byte size of the file.
    pub size: u64,
}

const V3_FILES: &[FileSpec] = &[
    FileSpec {
        hf_url: "https://huggingface.co/PalatineVision/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.int8.onnx",
        mirror_name: "parakeet-v3-encoder.int8.onnx",
        local_name: "encoder.int8.onnx",
        sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
        size: 652_183_999,
    },
    FileSpec {
        hf_url: "https://huggingface.co/PalatineVision/parakeet-tdt-0.6b-v3-onnx/resolve/main/decoder_joint-model.int8.onnx",
        mirror_name: "parakeet-v3-decoder_joint.int8.onnx",
        local_name: "decoder_joint.int8.onnx",
        sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
        size: 18_202_004,
    },
    FileSpec {
        hf_url: "https://huggingface.co/PalatineVision/parakeet-tdt-0.6b-v3-onnx/resolve/main/nemo128.onnx",
        mirror_name: "parakeet-v3-nemo128.onnx",
        local_name: "nemo128.onnx",
        sha256: "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f",
        size: 139_764,
    },
    FileSpec {
        hf_url: "https://huggingface.co/PalatineVision/parakeet-tdt-0.6b-v3-onnx/resolve/main/vocab.txt",
        mirror_name: "parakeet-v3-vocab.txt",
        local_name: "vocab.txt",
        sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
        size: 93_939,
    },
];

const V2_FILES: &[FileSpec] = &[
    FileSpec {
        hf_url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main/encoder-model.int8.onnx",
        mirror_name: "parakeet-v2-encoder.int8.onnx",
        local_name: "encoder.int8.onnx",
        sha256: "3e0581fda6ab843888b51e56d5ee78b6d5bc3237ec113af1f732d1d5286aa155",
        size: 652_184_014,
    },
    FileSpec {
        hf_url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main/decoder_joint-model.int8.onnx",
        mirror_name: "parakeet-v2-decoder_joint.int8.onnx",
        local_name: "decoder_joint.int8.onnx",
        sha256: "a449f49acd68979d418651dd2dcb737cc0f1bf0225e009e29ee326354edbf7d3",
        size: 8_998_286,
    },
    FileSpec {
        hf_url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main/nemo128.onnx",
        mirror_name: "parakeet-v2-nemo128.onnx",
        local_name: "nemo128.onnx",
        sha256: "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f",
        size: 139_764,
    },
    FileSpec {
        hf_url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main/vocab.txt",
        mirror_name: "parakeet-v2-vocab.txt",
        local_name: "vocab.txt",
        sha256: "ec182b70dd42113aff6c5372c75cac58c952443eb22322f57bbd7f53977d497d",
        size: 9_384,
    },
];

impl Model {
    /// Parse a CLI model name. Only the full registry ids are accepted.
    pub fn parse(name: &str) -> anyhow::Result<Model> {
        match name.trim().to_lowercase().as_str() {
            "parakeet-tdt-0.6b-v3-int8" => Ok(Model::ParakeetV3Int8),
            "parakeet-tdt-0.6b-v2-int8" => Ok(Model::ParakeetV2Int8),
            other => Err(anyhow!(
                "unknown model '{other}' (try: parakeet-tdt-0.6b-v3-int8, parakeet-tdt-0.6b-v2-int8)"
            )),
        }
    }

    /// Every known model, default first.
    pub fn all() -> [Model; 2] {
        [Model::ParakeetV3Int8, Model::ParakeetV2Int8]
    }

    /// The model `setup` installs when no `--model` is given.
    pub fn default_model() -> Model {
        Model::ParakeetV3Int8
    }

    /// Stable identifier used as the on-disk directory name.
    pub fn id(&self) -> &'static str {
        match self {
            Model::ParakeetV3Int8 => "parakeet-tdt-0.6b-v3-int8",
            Model::ParakeetV2Int8 => "parakeet-tdt-0.6b-v2-int8",
        }
    }

    pub fn size_hint(&self) -> &'static str {
        match self {
            Model::ParakeetV3Int8 => "~650MB",
            Model::ParakeetV2Int8 => "~631MB",
        }
    }

    /// Pinned file specs (source URLs, hashes, sizes) for this model.
    /// Local filenames are canonicalized so every Parakeet variant loads
    /// through the same code.
    pub fn specs(&self) -> &'static [FileSpec] {
        match self {
            Model::ParakeetV3Int8 => V3_FILES,
            Model::ParakeetV2Int8 => V2_FILES,
        }
    }

    /// `(huggingface_url, mirror_filename, local_filename)` triples, kept
    /// for callers that only need names and URLs.
    pub fn files(&self) -> Vec<(&'static str, &'static str, &'static str)> {
        self.specs()
            .iter()
            .map(|s| (s.hf_url, s.mirror_name, s.local_name))
            .collect()
    }

    /// Total pinned bytes across all files.
    pub fn total_size(&self) -> u64 {
        self.specs().iter().map(|s| s.size).sum()
    }

    /// Directory this model is (or will be) installed in.
    pub fn dir(&self) -> anyhow::Result<PathBuf> {
        Ok(models_root()?.join(self.id()))
    }

    /// Staging directory downloads land in before the atomic publish.
    fn staging_dir(&self) -> anyhow::Result<PathBuf> {
        Ok(models_root()?.join(format!(".{}.staging", self.id())))
    }

    /// Cross-process lock file guarding downloads of this model.
    fn lock_path(&self) -> anyhow::Result<PathBuf> {
        Ok(models_root()?.join(format!(".{}.lock", self.id())))
    }

    /// Whether every file for this model is present on disk with the pinned
    /// size. A cheap check — use [`Model::verify`] for the full hash check.
    pub fn is_ready(&self) -> bool {
        match self.dir() {
            Ok(dir) => self.specs().iter().all(|s| {
                s.size > 0
                    && fs::metadata(dir.join(s.local_name))
                        .map(|m| m.len())
                        .unwrap_or(0)
                        == s.size
            }),
            Err(_) => false,
        }
    }

    /// Full hash + size check over the installed files.
    pub fn verify(&self) -> anyhow::Result<()> {
        let dir = self.dir()?;
        for spec in self.specs() {
            download::verify_file(&dir.join(spec.local_name), spec.sha256, spec.size)?;
        }
        Ok(())
    }
}

/// Root directory holding all installed models. `JISUNO_MODELS_DIR` overrides
/// the platform data directory (used by tests and by anyone relocating
/// the ~650 MB per model).
pub fn models_root() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = std::env::var("JISUNO_MODELS_DIR") {
        if !dir.trim().is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let base =
        dirs::data_dir().context("could not determine a data directory for this platform")?;
    Ok(base.join("jisuno").join("models"))
}

/// Held while a model downloads: the lock file is removed on drop. Only one
/// holder per model across all processes.
pub struct ModelLock {
    path: PathBuf,
}

impl Drop for ModelLock {
    fn drop(&mut self) {
        fs::remove_file(&self.path).ok();
    }
}

/// Claim the download lock for `model`. Fails when another live process holds
/// it; reclaims the lock when the recorded pid is dead (stale).
pub fn acquire_lock(model: &Model) -> anyhow::Result<ModelLock> {
    let path = model.lock_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let me = std::process::id().to_string();
    for _ in 0..8 {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                writeln!(f, "{me}")?;
                f.sync_all().ok();
                return Ok(ModelLock { path });
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                let holder = fs::read_to_string(&path)
                    .unwrap_or_default()
                    .trim()
                    .parse::<u32>()
                    .ok();
                match holder {
                    Some(pid) if pid_alive(pid) => {
                        return Err(anyhow!(
                            "{} is busy (download in progress, pid {pid}); refusing to run concurrently",
                            model.id()
                        ));
                    }
                    // Stale lock (dead pid or unreadable): clear and retry.
                    _ => {
                        fs::remove_file(&path).ok();
                    }
                }
            }
            Err(e) => {
                return Err(e).with_context(|| format!("creating lock {}", path.display()));
            }
        }
    }
    Err(anyhow!(
        "could not acquire the download lock for {}",
        model.id()
    ))
}

/// Whether `pid` is a live process. Conservative on non-unix: any recorded
/// pid counts as live, so the lock is never reclaimed there.
fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) performs error checking without sending a signal.
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if r == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Atomically publish a verified staging dir as `dest`: any previous install
/// moves aside first, then `staging` renames into place. A rename is atomic,
/// so `dest` is either the old model or the new one — never a half model.
/// On failure `staging` is left untouched.
pub fn atomic_install(staging: &Path, dest: &Path) -> anyhow::Result<()> {
    if fs::symlink_metadata(dest).is_ok() {
        let backup = staging.with_extension("backup");
        if fs::symlink_metadata(&backup).is_ok() {
            remove_dir(&backup)?;
        }
        fs::rename(dest, &backup).with_context(|| format!("moving aside {}", dest.display()))?;
        if let Err(e) = fs::rename(staging, dest) {
            // Best effort: put the old install back.
            fs::rename(&backup, dest).ok();
            return Err(e).with_context(|| format!("publishing {}", dest.display()));
        }
        remove_dir(&backup)?;
    } else {
        fs::rename(staging, dest).with_context(|| format!("publishing {}", dest.display()))?;
    }
    if let Some(parent) = dest.parent() {
        #[cfg(unix)]
        if let Ok(dir) = fs::File::open(parent) {
            dir.sync_all().ok();
        }
    }
    Ok(())
}

fn remove_dir(path: &Path) -> anyhow::Result<()> {
    if fs::symlink_metadata(path).is_ok() {
        fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

/// Download (or resume) every file of `model` into its staging dir, verify
/// hashes, and atomically publish. `cancel` is checked between chunks; a set
/// flag aborts with `.part` files left for the next run to resume.
pub fn download_model(
    model: &Model,
    tls_opts: &TlsOptions,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(Progress),
) -> anyhow::Result<()> {
    let root = models_root()?;
    fs::create_dir_all(&root)
        .with_context(|| format!("failed to create models dir {}", root.display()))?;
    // Hold the lock for the whole fetch + publish.
    let _lock = acquire_lock(model)?;
    let staging = model.staging_dir()?;
    fs::create_dir_all(&staging)
        .with_context(|| format!("failed to create staging dir {}", staging.display()))?;

    let agent = crate::tls::build_agent(tls_opts)?;
    let overall_total = model.total_size();
    let mut overall_done = 0u64;

    for spec in model.specs() {
        let dest = staging.join(spec.local_name);
        if download::verify_file(&dest, spec.sha256, spec.size).is_ok() {
            overall_done += spec.size;
            on_progress(Progress {
                file: spec.local_name.to_owned(),
                bytes_done: spec.size,
                bytes_total: Some(spec.size),
                overall_done,
                overall_total,
            });
            continue;
        }
        let mirror_url = format!("{MIRROR_BASE}/{}", spec.mirror_name);
        let sources: [(&str, String); 2] = [
            ("huggingface", spec.hf_url.to_owned()),
            ("mirror", mirror_url),
        ];
        let sources_ref: Vec<(&str, &str)> = sources
            .iter()
            .map(|(label, url)| (*label, url.as_str()))
            .collect();
        {
            let overall = &mut overall_done;
            let mut wrap = |p: Progress| {
                on_progress(Progress {
                    file: p.file,
                    bytes_done: p.bytes_done,
                    bytes_total: p.bytes_total,
                    overall_done: *overall + p.bytes_done,
                    overall_total,
                });
            };
            download::download_file(
                &agent,
                &sources_ref,
                &dest,
                spec.sha256,
                spec.size,
                cancel,
                &mut wrap,
            )?;
        }
        overall_done += spec.size;
    }

    atomic_install(&staging, &model.dir()?)?;
    Ok(())
}

/// `jisuno setup --model <name>`: download a model if it isn't present.
/// Progress goes to **stderr**; exits 0 on success, 1 (via the caller) on error.
pub fn run(name: &str, tls_opts: &TlsOptions) -> anyhow::Result<()> {
    let model = Model::parse(name)?;
    if model.is_ready() {
        eprintln!("{} is already installed.", model.id());
        jisuno_logic::protocol::status("ready");
        return Ok(());
    }
    let dir = model.dir()?;
    eprintln!(
        "Setting up {} ({}) in {}",
        model.id(),
        model.size_hint(),
        dir.display()
    );
    download_with_progress(&model, tls_opts, &AtomicBool::new(false))?;
    eprintln!("Done. {} is ready.", model.id());
    jisuno_logic::protocol::status("ready");
    Ok(())
}

/// Fetch a model with the standard stderr progress bar, then announce readiness.
pub fn download_with_progress(
    model: &Model,
    tls_opts: &TlsOptions,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let mut progress = |p: Progress| print_progress(&p);
    download_model(model, tls_opts, cancel, &mut progress)?;
    model.verify()?;
    Ok(())
}

/// Remove an installed model. Refuses while another process holds the
/// download lock; errors when the model is not installed.
pub fn remove(model: &Model) -> anyhow::Result<()> {
    // Hold the lock through the removal so a concurrent download can't be
    // publishing underneath us. A live holder makes this fail first.
    let _lock = acquire_lock(model).map_err(|e| {
        anyhow!(
            "cannot remove {} while it is downloading: {e:#}",
            model.id()
        )
    })?;
    let dir = model.dir()?;
    if fs::symlink_metadata(&dir).is_err() {
        return Err(anyhow!("{} is not installed", model.id()));
    }
    remove_dir(&dir)?;
    // Drop leftover staging from an interrupted install, if any.
    remove_dir(&model.staging_dir()?)?;
    Ok(())
}

/// `jisuno models`: list every known model and whether it's installed.
pub fn list() -> anyhow::Result<()> {
    let root = models_root()?;
    eprintln!("Models directory: {}", root.display());
    for model in Model::all() {
        let mark = if model.is_ready() {
            "installed"
        } else {
            "not installed"
        };
        let default = if model == Model::default_model() {
            " (default)"
        } else {
            ""
        };
        println!(
            "{:<28} {:>8} [{}]{}",
            model.id(),
            human(model.total_size()),
            mark,
            default
        );
    }
    Ok(())
}

/// One stderr progress line per update, with an overall percentage across the
/// whole model. Keeps stdout clean for the line protocol.
pub fn print_progress(p: &Progress) {
    match p.bytes_total {
        Some(t) if t > 0 => {
            let pct = p.bytes_done as f64 / t as f64 * 100.0;
            if p.overall_total > 0 {
                let overall = p.overall_done as f64 / p.overall_total as f64 * 100.0;
                eprint!(
                    "\r  {:<24} {:>3.0}% ({} / {})  overall {:>3.0}%",
                    p.file,
                    pct,
                    human(p.bytes_done),
                    human(t),
                    overall
                );
            } else {
                eprint!(
                    "\r  {:<24} {:>3.0}% ({} / {})",
                    p.file,
                    pct,
                    human(p.bytes_done),
                    human(t)
                );
            }
        }
        _ => eprint!("\r  {:<24} {}", p.file, human(p.bytes_done)),
    }
    if p.bytes_total == Some(p.bytes_done) {
        eprintln!();
    }
    let _ = std::io::stderr().flush();
}

/// Render a byte count in human-friendly units.
fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_ids() {
        assert_eq!(
            Model::parse("parakeet-tdt-0.6b-v3-int8").unwrap(),
            Model::ParakeetV3Int8
        );
        assert_eq!(
            Model::parse("Parakeet-TDT-0.6B-V2-Int8").unwrap(),
            Model::ParakeetV2Int8
        );
        assert!(Model::parse("parakeet-v3").is_err());
        assert!(Model::parse("v3").is_err());
        assert!(Model::parse("nope").is_err());
    }

    #[test]
    fn registry_has_four_pinned_files_per_model() {
        for model in Model::all() {
            let specs = model.specs();
            assert_eq!(specs.len(), 4, "{}", model.id());
            for spec in specs {
                assert!(!spec.sha256.is_empty(), "{}", spec.local_name);
                assert_eq!(spec.sha256.len(), 64, "{}", spec.local_name);
                assert!(spec.size > 0, "{}", spec.local_name);
                assert!(!spec.hf_url.is_empty());
            }
        }
        assert_eq!(Model::ParakeetV3Int8.files().len(), 4);
    }

    #[test]
    fn human_units() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(2048), "2.0 KB");
        assert_eq!(human(5 * 1024 * 1024), "5.0 MB");
    }

    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn stale_lock_is_reclaimed_live_lock_is_not() {
        let _env = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("JISUNO_MODELS_DIR", tmp.path());
        let model = Model::ParakeetV3Int8;
        // Stale: a pid that cannot exist.
        fs::write(model.lock_path().unwrap(), "4194303\n").unwrap();
        assert!(!pid_alive(4194303));
        {
            let _guard = acquire_lock(&model).expect("stale lock reclaimed");
            assert!(model.lock_path().unwrap().exists());
        }
        assert!(!model.lock_path().unwrap().exists(), "lock removed on drop");
        // Live: our own process.
        {
            let _guard = acquire_lock(&model).expect("fresh acquire");
            assert!(acquire_lock(&model).is_err(), "live lock refuses");
        }
        std::env::remove_var("JISUNO_MODELS_DIR");
    }

    #[test]
    fn publish_replaces_old_install_and_never_leaves_half_model() {
        let tmp = tempfile::tempdir().unwrap();
        // Publish over an existing install: the old dir moves aside, the
        // new staging takes its place, no backup litter remains.
        let staging = tmp.path().join(".x.staging");
        let dest = tmp.path().join("x");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("f"), b"new").unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("old"), b"old").unwrap();
        atomic_install(&staging, &dest).expect("publish over old");
        assert!(!staging.exists());
        assert_eq!(fs::read(dest.join("f")).unwrap(), b"new");
        assert!(!dest.join("old").exists());
        assert!(!tmp.path().join(".x.backup").exists());
        // Failure (dest parent missing): staging is left untouched.
        let staging2 = tmp.path().join(".y.staging");
        let dest2 = tmp.path().join("missing").join("y");
        fs::create_dir_all(&staging2).unwrap();
        fs::write(staging2.join("f"), b"new").unwrap();
        assert!(atomic_install(&staging2, &dest2).is_err());
        assert!(staging2.exists(), "staging untouched");
        assert!(!dest2.exists());
    }
}
