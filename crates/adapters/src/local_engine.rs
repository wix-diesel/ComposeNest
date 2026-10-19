//! A bounded local Engine request for secret-bearing standalone container configuration.

use crate::docker_create::CreateDockerError as Error;
use serde_json::Value;
use std::{ffi::OsStr, io, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const RESPONSE_LIMIT: u64 = 64 * 1024;

pub(crate) async fn create(
    endpoint: &OsStr,
    name: &str,
    platform: &str,
    config: &Value,
) -> Result<(), Error> {
    if !composenest_domain::compose::valid_container_name(name)
        || !matches!(platform, "linux/amd64" | "linux/arm64")
    {
        return Err(Error::InvalidInput);
    }
    let body = serde_json::to_vec(config).map_err(|_| Error::InvalidInput)?;
    let path = format!(
        "/v1.52/containers/create?name={name}&platform={}",
        platform.replace('/', "%2F")
    );
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tokio::time::timeout(Duration::from_secs(120), async {
        let stream = connect(endpoint).await.map_err(|_| Error::Unavailable)?;
        exchange(stream, request.as_bytes(), &body).await
    })
    .await
    .map_err(|_| Error::OutcomeUnknown)?
}

#[cfg(windows)]
async fn connect(endpoint: &OsStr) -> io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    let text = endpoint
        .to_str()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let pipe = text
        .strip_prefix("npipe:////./pipe/")
        .filter(|pipe| !pipe.is_empty())
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    loop {
        match tokio::net::windows::named_pipe::ClientOptions::new()
            .open(format!(r"\\.\pipe\{pipe}"))
        {
            Err(error) if error.raw_os_error() == Some(231) => {
                tokio::time::sleep(Duration::from_millis(20)).await
            }
            result => return result,
        }
    }
}

#[cfg(unix)]
async fn connect(endpoint: &OsStr) -> io::Result<tokio::net::UnixStream> {
    let path = endpoint
        .to_str()
        .and_then(|endpoint| endpoint.strip_prefix("unix://"))
        .filter(|path| path.starts_with('/'))
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    tokio::net::UnixStream::connect(path).await
}

async fn exchange(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    header: &[u8],
    body: &[u8],
) -> Result<(), Error> {
    stream
        .write_all(header)
        .await
        .map_err(|_| Error::OutcomeUnknown)?;
    stream
        .write_all(body)
        .await
        .map_err(|_| Error::OutcomeUnknown)?;
    stream.flush().await.map_err(|_| Error::OutcomeUnknown)?;
    let mut response = Vec::new();
    stream
        .take(RESPONSE_LIMIT + 1)
        .read_to_end(&mut response)
        .await
        .map_err(|_| Error::OutcomeUnknown)?;
    if response.len() as u64 > RESPONSE_LIMIT {
        return Err(Error::OutcomeUnknown);
    }
    if !response.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        return Err(Error::OutcomeUnknown);
    }
    let status = response
        .split(|byte| *byte == b'\n')
        .next()
        .and_then(|line| std::str::from_utf8(line).ok())
        .and_then(|line| {
            line.strip_prefix("HTTP/1.1 ")
                .or_else(|| line.strip_prefix("HTTP/1.0 "))
        })
        .and_then(|line| line.split_whitespace().next())
        .and_then(|code| code.parse::<u16>().ok());
    match status {
        Some(201) => Ok(()),
        Some(409) => Err(Error::ExistingContainer),
        Some(400 | 404) => Err(Error::InvalidInput),
        _ => Err(Error::OutcomeUnknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_body_stays_private_and_transport_failures_never_report_success() {
        for (response, expected) in [
            (
                "HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\n{}",
                Ok(()),
            ),
            (
                "HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\n\r\n",
                Err(Error::ExistingContainer),
            ),
            (
                "HTTP/1.1 500 Error\r\nContent-Length: 0\r\n\r\n",
                Err(Error::OutcomeUnknown),
            ),
            ("", Err(Error::OutcomeUnknown)),
        ] {
            let (client, mut server) = tokio::io::duplex(1024);
            let writer = tokio::spawn(async move {
                let mut body = [0; 6];
                server.read_exact(&mut body).await.unwrap();
                assert_eq!(&body, b"secret");
                server.write_all(response.as_bytes()).await.unwrap();
            });
            assert_eq!(exchange(client, b"", b"secret").await, expected);
            writer.await.unwrap();
        }
    }
}
