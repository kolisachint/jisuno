//! Control socket protocol (`jisuno model ...`): one-line JSON requests and
//! responses over a Unix domain socket.
//!
//! Not app-gated, so the CLI builds on Linux: the client connects, sends one
//! [`Request`] line and reads one [`Response`] line. Any connect failure
//! means the app is not running ([`ClientError::NotRunning`]). The server
//! half lives in `app::socket` (macOS only).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Socket timeout for one request: the server answers from a snapshot, so
/// this only fires when the app is wedged.
const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// One-line JSON request. `{"cmd":"status"}` and friends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Request {
    Status,
    Warm,
    Unload,
}

/// One-line JSON response. `status` fills every field;
/// `warm`/`unload` answer `{ok:true}`; failures answer `{ok:false,error}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn status(model: String, state: &str, rss_mb: u64) -> Self {
        Self {
            ok: true,
            model: Some(model),
            state: Some(state.to_string()),
            rss_mb: Some(rss_mb),
            error: None,
        }
    }

    pub fn ok() -> Self {
        Self {
            ok: true,
            model: None,
            state: None,
            rss_mb: None,
            error: None,
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            model: None,
            state: None,
            rss_mb: None,
            error: Some(message.into()),
        }
    }

    /// The request line: JSON plus `\n`.
    pub fn encode_line(&self) -> String {
        let mut line =
            serde_json::to_string(self).unwrap_or_else(|_| r#"{"ok":false}"#.to_string());
        line.push('\n');
        line
    }
}

impl Request {
    /// The request line: JSON plus `\n`.
    pub fn encode_line(&self) -> String {
        let mut line =
            serde_json::to_string(self).unwrap_or_else(|_| r#"{"cmd":"status"}"#.to_string());
        line.push('\n');
        line
    }

    /// Parse one request line (trailing newline tolerated).
    pub fn decode_line(line: &str) -> Result<Self, String> {
        serde_json::from_str(line.trim()).map_err(|e| format!("bad request: {e}"))
    }
}

impl Response {
    /// Parse one response line (trailing newline tolerated).
    pub fn decode_line(line: &str) -> Result<Self, String> {
        serde_json::from_str(line.trim()).map_err(|e| format!("bad response: {e}"))
    }
}

/// `~/Library/Application Support/jisuno/jisuno.sock` on every OS (the CLI
/// builds on Linux; a missing socket just means "not running").
pub fn socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/jisuno/jisuno.sock")
}

/// Why a `model` command failed.
#[derive(Debug)]
pub enum ClientError {
    /// No socket: the app is not running.
    NotRunning,
    /// Connected but the exchange failed.
    Failed(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning => write!(f, "Jisuno is not running"),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

/// Send one request, read one response. Any connect failure — missing file,
/// stale socket, refused — is [`ClientError::NotRunning`].
pub fn send(request: &Request) -> Result<Response, ClientError> {
    let stream = UnixStream::connect(socket_path()).map_err(|_| ClientError::NotRunning)?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .and_then(|()| stream.set_read_timeout(Some(IO_TIMEOUT)))
        .map_err(|e| ClientError::Failed(format!("{e:#}")))?;
    let mut stream = stream;
    stream
        .write_all(request.encode_line().as_bytes())
        .map_err(|e| ClientError::Failed(format!("{e:#}")))?;
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|e| ClientError::Failed(format!("{e:#}")))?;
    Response::decode_line(&line).map_err(ClientError::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_lines() {
        assert_eq!(Request::Status.encode_line(), "{\"cmd\":\"status\"}\n");
        assert_eq!(Request::Warm.encode_line(), "{\"cmd\":\"warm\"}\n");
        assert_eq!(Request::Unload.encode_line(), "{\"cmd\":\"unload\"}\n");
        assert_eq!(
            Request::decode_line("{\"cmd\":\"warm\"}\n"),
            Ok(Request::Warm)
        );
        assert!(Request::decode_line("{\"cmd\":\"nuke\"}").is_err());
        assert!(Request::decode_line("not json").is_err());
    }

    #[test]
    fn response_round_trip() {
        let status = Response::status("parakeet-tdt-0.6b-v3-int8".to_string(), "warm", 912);
        let line = status.encode_line();
        assert_eq!(
            line,
            "{\"ok\":true,\"model\":\"parakeet-tdt-0.6b-v3-int8\",\"state\":\"warm\",\"rss_mb\":912}\n"
        );
        assert_eq!(Response::decode_line(&line), Ok(status));

        let ok = Response::ok();
        assert_eq!(ok.encode_line(), "{\"ok\":true}\n");
        assert_eq!(Response::decode_line("{\"ok\":true}\n"), Ok(ok));

        let err = Response::err("model not installed");
        assert_eq!(
            err.encode_line(),
            "{\"ok\":false,\"error\":\"model not installed\"}\n"
        );
        assert_eq!(Response::decode_line(&err.encode_line()), Ok(err));
    }

    #[test]
    fn socket_path_shape() {
        let path = socket_path();
        assert!(path.ends_with("Library/Application Support/jisuno/jisuno.sock"));
    }
}
