//! Resumable, hash-verified file downloads.
//!
//! [`download_file`] fetches one file from an ordered list of mirror sources
//! into `dest`, verifying its size and sha256. Downloads stream through a
//! `<dest>.part` file so an interrupted run never looks complete: a later run
//! resumes with an HTTP Range request when the server honors it (and restarts
//! cleanly when it does not). A hash or size mismatch deletes the partial
//! file and falls through to the next source.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, Context};
use sha2::{Digest, Sha256};

/// Progress update for one file inside a larger (possibly multi-file) fetch.
pub struct Progress {
    /// Local filename being fetched (e.g. `encoder.int8.onnx`).
    pub file: String,
    /// Bytes written for this file so far (includes any resumed prefix).
    pub bytes_done: u64,
    /// Total bytes expected for this file, when the server reports one.
    pub bytes_total: Option<u64>,
    /// Bytes written across all files so far.
    pub overall_done: u64,
    /// Total bytes expected across all files.
    pub overall_total: u64,
}

/// Download one file, trying each `(label, url)` source in order.
///
/// * Streams to `<dest>.part`, resuming a previous partial when the server
///   answers `206 Partial Content` to a Range request.
/// * Hashes bytes while writing (re-hashing an existing partial first), then
///   verifies size and sha256 before fsync + rename into place.
/// * A size/hash mismatch deletes the partial and tries the next source.
/// * Checks `cancel` between chunks; a set flag aborts with an error and
///   leaves the `.part` file for a later run to resume.
///
/// The final error names the file and, on a hash mismatch, the expected and
/// actual digests.
pub fn download_file(
    agent: &ureq::Agent,
    sources: &[(&str, &str)],
    dest: &Path,
    expected_sha256: &str,
    expected_size: u64,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(Progress),
) -> anyhow::Result<()> {
    let file = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dest.display().to_string());
    if sources.is_empty() {
        return Err(anyhow!("no download sources for {file}"));
    }

    let part = part_path(dest);
    let mut last_err = None;
    for (label, url) in sources {
        match attempt(agent, url, dest, &file, expected_size, cancel, on_progress) {
            Ok(hasher) => {
                let actual = hex(&hasher.finalize());
                if !actual.eq_ignore_ascii_case(expected_sha256) {
                    // Verified before rename, so only the .part needs deleting;
                    // `dest` is untouched and the next source starts clean.
                    fs::remove_file(&part).ok();
                    last_err = Some(anyhow!(
                        "{file}: sha256 mismatch (expected {expected_sha256}, got {actual})"
                    ));
                    continue;
                }
                finalize(&part, dest)?;
                emit(&file, expected_size, Some(expected_size), on_progress);
                return Ok(());
            }
            Err(e) if cancel.load(Ordering::Relaxed) => {
                return Err(anyhow!("{file}: download cancelled ({label})").context(e));
            }
            Err(e) => {
                last_err = Some(anyhow!("{label} failed for {file}: {e:#}"));
            }
        }
    }
    Err(anyhow!("all sources failed for {file}")
        .context(last_err.unwrap_or_else(|| anyhow!("no sources tried"))))
}

/// One attempt against a single URL: resume-or-restart, stream, size-check,
/// fsync + rename. Returns the running sha256 over the final bytes.
fn attempt(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    file: &str,
    expected_size: u64,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(Progress),
) -> anyhow::Result<Sha256> {
    let part = part_path(dest);
    let existing = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);

    // A partial exactly the expected size might already be complete (e.g. a
    // previous run wrote everything but was killed before rename): hash it
    // in place and hand it back without any network use. The caller checks
    // the digest and deletes the part on mismatch.
    if existing == expected_size && expected_size > 0 {
        return hash_file(&part);
    }
    if existing > expected_size {
        fs::remove_file(&part).with_context(|| format!("removing oversized {}", part.display()))?;
    }

    // With a usable partial, first try a Range resume; fall back to a full
    // fetch when the server ignores Range (200) or rejects the range (416).
    // The response is consumed by `stream`, so the handshake and the body
    // read live in one place.
    if existing > 0 && existing < expected_size {
        let resume = agent
            .get(url)
            .set("Range", &format!("bytes={existing}-"))
            .call();
        match resume {
            Ok(resp) if resp.status() == 206 => {
                let hasher = hash_file(&part)?;
                let (hasher, written) =
                    stream(resp, &part, file, existing, cancel, hasher, on_progress)?;
                return checked(hasher, written, &part, file, expected_size);
            }
            Ok(resp) => {
                // Server ignored Range: restart from scratch.
                let (hasher, written) =
                    stream(resp, &part, file, 0, cancel, Sha256::new(), on_progress)?;
                return checked(hasher, written, &part, file, expected_size);
            }
            Err(ureq::Error::Status(416, _)) => {
                fs::remove_file(&part).with_context(|| format!("removing {}", part.display()))?;
            }
            Err(e) => return Err(e).with_context(|| format!("request failed: {url}")),
        }
    }

    let resp = agent
        .get(url)
        .call()
        .with_context(|| format!("request failed: {url}"))?;
    let (hasher, written) = stream(resp, &part, file, 0, cancel, Sha256::new(), on_progress)?;
    checked(hasher, written, &part, file, expected_size)
}

/// Size-check streamed bytes: a short or over-long body means this source is
/// corrupt, so delete the part and report (the next source starts clean).
fn checked(
    hasher: Sha256,
    written: u64,
    part: &Path,
    file: &str,
    expected_size: u64,
) -> anyhow::Result<Sha256> {
    if written != expected_size {
        fs::remove_file(part).ok();
        return Err(anyhow!(
            "{file}: size mismatch (expected {expected_size} bytes, got {written})"
        ));
    }
    Ok(hasher)
}

/// Stream one response body into `part` (appending when `start > 0`),
/// hashing as we go. `hasher` must already cover `part[..start]`. Returns
/// the hasher and the total bytes now in `part`. Verification and rename
/// happen in the caller so a bad source never touches `dest`.
fn stream(
    resp: ureq::Response,
    part: &Path,
    file: &str,
    start: u64,
    cancel: &AtomicBool,
    mut hasher: Sha256,
    on_progress: &mut dyn FnMut(Progress),
) -> anyhow::Result<(Sha256, u64)> {
    let total: Option<u64> = resp.header("content-length").and_then(|v| v.parse().ok());
    let mut out = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(start == 0)
        .append(start > 0)
        .open(part)
        .with_context(|| format!("creating {}", part.display()))?;
    // When restarting, the file must actually be empty (a 200 to a Range
    // request still leaves the old partial on disk).
    if start == 0 {
        out.set_len(0)?;
    }

    let mut reader = resp.into_reader();
    let mut written = start;
    let mut buf = [0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("{file}: cancelled after {written} bytes"));
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        written += n as u64;
        emit(file, written, total.map(|t| t + start), on_progress);
    }
    drop(out);
    Ok((hasher, written))
}

/// `<dest>.part` sidecar path.
fn part_path(dest: &Path) -> std::path::PathBuf {
    dest.with_extension("part")
}

/// Lowercase hex of a digest.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// sha256 over the whole file at `path`.
fn hash_file(path: &Path) -> anyhow::Result<Sha256> {
    let mut file = fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher)
}

/// Verify an on-disk file against its pinned size and sha256.
pub fn verify_file(path: &Path, expected_sha256: &str, expected_size: u64) -> anyhow::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let actual_size = fs::metadata(path)
        .with_context(|| format!("reading {name}"))?
        .len();
    if actual_size != expected_size {
        return Err(anyhow!(
            "{name}: size mismatch (expected {expected_size} bytes, got {actual_size})"
        ));
    }
    let actual = hex(&hash_file(path)?.finalize());
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(anyhow!(
            "{name}: sha256 mismatch (expected {expected_sha256}, got {actual})"
        ));
    }
    Ok(())
}

/// Emit one progress update.
fn emit(
    file: &str,
    bytes_done: u64,
    bytes_total: Option<u64>,
    on_progress: &mut dyn FnMut(Progress),
) {
    on_progress(Progress {
        file: file.to_owned(),
        bytes_done,
        bytes_total,
        overall_done: bytes_done,
        overall_total: bytes_total.unwrap_or(bytes_done),
    });
}

/// fsync the part file and atomically rename it into place. Call only after
/// the size and hash are verified: a rename is forever.
fn finalize(part: &Path, dest: &Path) -> anyhow::Result<()> {
    let f = fs::File::open(part).with_context(|| format!("reading {}", part.display()))?;
    f.sync_all()
        .with_context(|| format!("fsync {}", part.display()))?;
    drop(f);
    fs::rename(part, dest).with_context(|| format!("finalizing {}", dest.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Deterministic pseudo-random blob.
    fn blob(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| (i.wrapping_mul(2654435761) ^ (seed as usize)) as u8)
            .collect()
    }

    fn digest(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        hex(&h.finalize())
    }

    fn agent() -> ureq::Agent {
        // A hung request fails the test instead of blocking the run.
        ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(30))
            .build()
    }

    /// Localhost HTTP/1.1 server. Serves `routes` (path -> body); when
    /// `honor_range`, answers `Range: bytes=N-` with 206. Every real GET
    /// records its Range header (or None) in `seen`. Accepts connections
    /// until the returned [`TestServer`] is joined or dropped. Every
    /// response carries `Connection: close`, so the client never pools a
    /// socket across requests.
    fn spawn_server(
        routes: HashMap<String, (Vec<u8>, bool)>,
        seen: Arc<Mutex<Vec<Option<String>>>>,
    ) -> (String, TestServer) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("test server addr");
        listener.set_nonblocking(true).expect("nonblocking");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                let stream = match listener.accept() {
                    Ok((stream, _peer)) => stream,
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                };
                // Accepted sockets inherit O_NONBLOCK from the listener on
                // macOS/BSD. Left set, the request read races the client's
                // send and fails with WouldBlock, the live request gets
                // dropped, and the client sees a connection reset.
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                serve_connection(stream, &routes, &seen);
            }
        });
        (
            format!("http://{addr}"),
            TestServer {
                stop,
                handle: Some(handle),
            },
        )
    }

    /// Handle to a [`spawn_server`] thread. `join` (or drop) stops the
    /// accept loop and waits for the thread to exit.
    struct TestServer {
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl TestServer {
        fn join(mut self) -> std::thread::Result<()> {
            self.stop.store(true, Ordering::Relaxed);
            self.handle.take().map_or(Ok(()), |h| h.join())
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    /// Read one request head from `stream` and answer it. A connection that
    /// closes before a request line is a stray probe: it is dropped without
    /// being counted.
    fn serve_connection(
        stream: TcpStream,
        routes: &HashMap<String, (Vec<u8>, bool)>,
        seen: &Mutex<Vec<Option<String>>>,
    ) {
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        match reader.read_line(&mut request_line) {
            Ok(n) if n > 0 => {}
            _ => return,
        }
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let path = parts.next().unwrap_or("/").to_owned();
        let mut range: Option<String> = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("range") {
                    range = Some(value.trim().to_owned());
                }
            }
        }
        let out = reader.get_mut();
        if method != "GET" {
            let _ = out.write_all(
                b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        }
        seen.lock().expect("seen").push(range.clone());
        let Some((body, honor)) = routes.get(&path) else {
            let _ = out.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        };
        let partial = match range.as_deref().and_then(parse_range_start) {
            Some(start) if *honor && start < body.len() => Some(start),
            _ => None,
        };
        let head = match partial {
            Some(start) => format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len() - 1,
                body.len(),
                body.len() - start
            ),
            None => format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ),
        };
        let _ = out.write_all(head.as_bytes());
        let _ = out.write_all(&body[partial.unwrap_or(0)..]);
    }

    /// Start offset from a `bytes=N-` Range header.
    fn parse_range_start(range: &str) -> Option<usize> {
        range
            .strip_prefix("bytes=")?
            .strip_suffix('-')?
            .parse()
            .ok()
    }

    fn route(blob: Vec<u8>, honor_range: bool) -> HashMap<String, (Vec<u8>, bool)> {
        let mut m = HashMap::new();
        m.insert("/file".to_string(), (blob, honor_range));
        m
    }

    fn no_progress(_: Progress) {}

    static NET_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Hold while a test talks to a localhost server: clears proxy env vars
    /// (ureq honors them, which would route localhost through a proxy) and
    /// restores them on drop so parallel tests never see a half-set env.
    struct ProxyGuard {
        saved: Vec<(String, String)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl ProxyGuard {
        fn hold() -> Self {
            let lock = NET_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let keys = [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ];
            let mut saved = Vec::new();
            for k in keys {
                if let Ok(v) = std::env::var(k) {
                    saved.push((k.to_string(), v));
                    std::env::remove_var(k);
                }
            }
            ProxyGuard { saved, _lock: lock }
        }
    }

    impl Drop for ProxyGuard {
        fn drop(&mut self) {
            for (k, v) in self.saved.drain(..) {
                std::env::set_var(k, v);
            }
        }
    }

    #[test]
    fn full_download_verifies_hash() {
        let _proxy = ProxyGuard::hold();
        let data = blob(300_000, 7);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (base, server) = spawn_server(route(data.clone(), true), Arc::clone(&seen));
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("model.onnx");
        let cancel = AtomicBool::new(false);
        let mut progress = Vec::new();
        download_file(
            &agent(),
            &[("test", &format!("{base}/file"))],
            &dest,
            &digest(&data),
            data.len() as u64,
            &cancel,
            &mut |p: Progress| {
                progress.push((p.bytes_done, p.bytes_total));
            },
        )
        .expect("download");
        assert_eq!(fs::read(&dest).expect("read dest"), data);
        assert!(!part_path(&dest).exists(), "no .part left behind");
        assert!(!progress.is_empty(), "progress reported");
        let (done, total) = progress.last().copied().unwrap();
        assert_eq!(done, data.len() as u64);
        assert_eq!(total, Some(data.len() as u64));
        server.join().expect("server");
    }

    #[test]
    fn resume_with_range() {
        let _proxy = ProxyGuard::hold();
        let data = blob(300_000, 13);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (base, server) = spawn_server(route(data.clone(), true), Arc::clone(&seen));
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("model.onnx");
        // Simulate an interrupted run: first half sits in the .part file.
        fs::write(part_path(&dest), &data[..150_000]).expect("write part");
        let cancel = AtomicBool::new(false);
        download_file(
            &agent(),
            &[("test", &format!("{base}/file"))],
            &dest,
            &digest(&data),
            data.len() as u64,
            &cancel,
            &mut no_progress,
        )
        .expect("resume");
        assert_eq!(fs::read(&dest).expect("read dest"), data);
        let seen = seen.lock().expect("seen");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].as_deref(), Some("bytes=150000-"), "Range resume");
        server.join().expect("server");
    }

    #[test]
    fn restart_when_server_ignores_range() {
        let _proxy = ProxyGuard::hold();
        let data = blob(200_000, 29);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (base, server) = spawn_server(route(data.clone(), false), Arc::clone(&seen));
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("model.onnx");
        // Garbage prefix: appending to it could never verify, so success
        // proves the client restarted from scratch on the 200 response.
        fs::write(part_path(&dest), vec![0xAA; 100_000]).expect("write part");
        let cancel = AtomicBool::new(false);
        download_file(
            &agent(),
            &[("test", &format!("{base}/file"))],
            &dest,
            &digest(&data),
            data.len() as u64,
            &cancel,
            &mut no_progress,
        )
        .expect("restart");
        assert_eq!(fs::read(&dest).expect("read dest"), data);
        server.join().expect("server");
    }

    #[test]
    fn hash_mismatch_falls_through_to_next_source() {
        let _proxy = ProxyGuard::hold();
        let bad = blob(120_000, 1);
        let good = blob(120_000, 2);
        let seen_bad = Arc::new(Mutex::new(Vec::new()));
        let seen_good = Arc::new(Mutex::new(Vec::new()));
        let (bad_base, bad_server) = spawn_server(route(bad, false), Arc::clone(&seen_bad));
        let (good_base, good_server) =
            spawn_server(route(good.clone(), true), Arc::clone(&seen_good));
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("model.onnx");
        let cancel = AtomicBool::new(false);
        download_file(
            &agent(),
            &[
                ("bad", &format!("{bad_base}/file")),
                ("good", &format!("{good_base}/file")),
            ],
            &dest,
            &digest(&good),
            good.len() as u64,
            &cancel,
            &mut no_progress,
        )
        .expect("fall through");
        assert_eq!(fs::read(&dest).expect("read dest"), good);
        assert!(!part_path(&dest).exists(), "bad partial deleted");
        bad_server.join().expect("bad server");
        good_server.join().expect("good server");
    }

    #[test]
    fn all_bad_error_names_file_and_hashes() {
        let _proxy = ProxyGuard::hold();
        let bad = blob(50_000, 3);
        let good = blob(50_000, 4);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (base, server) = spawn_server(route(bad.clone(), true), Arc::clone(&seen));
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("encoder.int8.onnx");
        let cancel = AtomicBool::new(false);
        let err = download_file(
            &agent(),
            &[("test", &format!("{base}/file"))],
            &dest,
            &digest(&good),
            good.len() as u64,
            &cancel,
            &mut no_progress,
        )
        .expect_err("must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("encoder.int8.onnx"), "names file: {msg}");
        assert!(msg.contains(&digest(&good)), "names expected hash: {msg}");
        assert!(msg.contains(&digest(&bad)), "names actual hash: {msg}");
        assert!(!dest.exists() && !part_path(&dest).exists());
        server.join().expect("server");
    }

    #[test]
    fn cancel_aborts_and_keeps_part() {
        let _proxy = ProxyGuard::hold();
        let data = blob(500_000, 5);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (base, server) = spawn_server(route(data.clone(), true), Arc::clone(&seen));
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("model.onnx");
        let cancel = AtomicBool::new(false);
        let cancel_ref = &cancel;
        let err = download_file(
            &agent(),
            &[("test", &format!("{base}/file"))],
            &dest,
            &digest(&data),
            data.len() as u64,
            &cancel,
            &mut move |p: Progress| {
                if p.bytes_done > 0 {
                    cancel_ref.store(true, Ordering::Relaxed);
                }
            },
        )
        .expect_err("must cancel");
        assert!(format!("{err:#}").contains("cancelled"), "{err:#}");
        assert!(!dest.exists(), "dest never finalized");
        assert!(part_path(&dest).exists(), ".part kept for resume");
        server.join().expect("server");
    }

    #[test]
    fn verify_file_checks_size_and_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("vocab.txt");
        fs::write(&path, b"hello").expect("write");
        verify_file(&path, &digest(b"hello"), 5).expect("ok");
        assert!(verify_file(&path, &digest(b"hello"), 6).is_err(), "size");
        assert!(verify_file(&path, &digest(b"other"), 5).is_err(), "hash");
    }
}
