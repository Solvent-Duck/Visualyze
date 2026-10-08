//! The local viewer protocol: newline-delimited JSON requests and responses over a
//! per-user Unix socket. The first request on every connection is `hello` with the
//! session token, which only the user can read. Models travel as JSON text and the
//! viewer validates them again; nothing here is trusted because it is local.
use std::{
    fs,
    io::{self, Read},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL: &str = "visualyze.ipc.v1";
/// A 2 MiB model escaped as a JSON string, plus the envelope.
pub const MAX_LINE: usize = 6 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    /// Validation diagnostics or other structured detail.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(id: u64, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::err_with(id, code, message, Value::Null)
    }
    pub fn err_with(
        id: u64,
        code: impl Into<String>,
        message: impl Into<String>,
        details: Value,
    ) -> Self {
        Self {
            id,
            result: None,
            error: Some(ErrorBody {
                code: code.into(),
                message: message.into(),
                details,
            }),
        }
    }
}

/// `VISUALYZE_RUN_DIR`, or the per-user application support directory.
pub fn run_dir() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("VISUALYZE_RUN_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(PathBuf::from(home).join("Library/Application Support/Visualyze/run"))
}

pub fn socket_path(dir: &Path) -> PathBuf {
    dir.join("viewer.sock")
}

/// Unix socket paths are limited to about 104 bytes on macOS.
pub const MAX_SOCKET_PATH: usize = 103;

pub fn check_socket_path(path: &Path) -> io::Result<()> {
    if path.as_os_str().len() > MAX_SOCKET_PATH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "socket path {} is too long for a Unix socket; set VISUALYZE_RUN_DIR to a shorter directory",
                path.display()
            ),
        ));
    }
    Ok(())
}

pub fn token_path(dir: &Path) -> PathBuf {
    dir.join("viewer.token")
}

/// Create the run directory readable only by this user.
pub fn prepare_dir(dir: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// A fresh random session token, written so only this user can read it.
pub fn issue_token(dir: &Path) -> io::Result<String> {
    let mut bytes = [0u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let path = token_path(dir);
    let _ = fs::remove_file(&path);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    io::Write::write_all(&mut file, token.as_bytes())?;
    Ok(token)
}

pub fn read_token(dir: &Path) -> io::Result<String> {
    Ok(fs::read_to_string(token_path(dir))?.trim().to_owned())
}

/// Compare without an early exit, so timing does not reveal a prefix.
pub fn token_matches(expected: &str, supplied: &str) -> bool {
    expected.len() == supplied.len()
        && expected
            .bytes()
            .zip(supplied.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_round_trip_and_omit_empty_fields() {
        let ok = serde_json::to_string(&Response::ok(3, serde_json::json!({"a": 1}))).unwrap();
        assert_eq!(ok, r#"{"id":3,"result":{"a":1}}"#);
        let err = serde_json::to_string(&Response::err(4, "stale_revision", "old")).unwrap();
        assert_eq!(
            err,
            r#"{"id":4,"error":{"code":"stale_revision","message":"old"}}"#
        );
        let back: Response = serde_json::from_str(&err).unwrap();
        assert_eq!(back.error.unwrap().code, "stale_revision");
    }

    #[test]
    fn tokens_are_private_random_and_compared_exactly() {
        let dir = std::env::temp_dir().join(format!("visualyze-token-{}", std::process::id()));
        prepare_dir(&dir).unwrap();
        let token = issue_token(&dir).unwrap();
        assert_eq!(token.len(), 32);
        assert_eq!(read_token(&dir).unwrap(), token);
        let mode = fs::metadata(token_path(&dir)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_ne!(issue_token(&dir).unwrap(), token);
        assert!(token_matches("abcd", "abcd"));
        assert!(!token_matches("abcd", "abce"));
        assert!(!token_matches("abcd", "abc"));
        fs::remove_dir_all(dir).unwrap();
    }
}
