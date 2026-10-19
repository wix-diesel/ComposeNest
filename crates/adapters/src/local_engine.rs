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

    async fn exchange_response(response: Vec<u8>) -> Result<(), Error> {
        let (client, mut server) = tokio::io::duplex(8);
        let writer = tokio::spawn(async move {
            let mut request = [0; 12];
            server.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"headersecret");
            for fragment in response.chunks(7) {
                server.write_all(fragment).await.unwrap();
            }
        });
        let result = exchange(client, b"header", b"secret").await;
        writer.await.unwrap();
        result
    }

    #[tokio::test]
    async fn fragmented_responses_preserve_creation_and_rejection_results() {
        for (status, expected) in [
            ("201 Created", Ok(())),
            ("400 Bad Request", Err(Error::InvalidInput)),
            ("404 Not Found", Err(Error::InvalidInput)),
            ("409 Conflict", Err(Error::ExistingContainer)),
            ("503 Unavailable", Err(Error::OutcomeUnknown)),
        ] {
            for version in ["HTTP/1.0", "HTTP/1.1"] {
                let response = format!("{version} {status}\r\nContent-Length: 0\r\n\r\n");
                assert_eq!(exchange_response(response.into_bytes()).await, expected);
            }
        }
    }

    #[tokio::test]
    async fn incomplete_or_invalid_http_responses_never_report_creation() {
        for response in [
            "HTTP/1.1 201 Created\r\nContent-Length: 0\r\n",
            "HTTP/2 201 Created\r\n\r\n",
            "HTTP/1.1 success\r\n\r\n",
            "201 Created\r\n\r\n",
            "\r\n\r\n",
        ] {
            assert_eq!(
                exchange_response(response.as_bytes().to_vec()).await,
                Err(Error::OutcomeUnknown)
            );
        }
    }

    #[tokio::test]
    async fn response_limit_is_inclusive_and_oversized_success_is_unknown() {
        for (length, expected) in [
            (RESPONSE_LIMIT, Ok(())),
            (RESPONSE_LIMIT + 1, Err(Error::OutcomeUnknown)),
        ] {
            let mut response = b"HTTP/1.1 201 Created\r\nConnection: close\r\n\r\n".to_vec();
            response.resize(length as usize, b' ');
            assert_eq!(exchange_response(response).await, expected);
        }
    }

    #[tokio::test]
    async fn disconnected_transport_leaves_the_creation_outcome_unknown() {
        let (client, server) = tokio::io::duplex(8);
        drop(server);
        assert_eq!(
            exchange(client, b"header", b"secret").await,
            Err(Error::OutcomeUnknown)
        );
    }

    #[tokio::test]
    async fn invalid_names_and_platforms_are_rejected_before_connecting() {
        for (name, platform) in [
            ("db/other", "linux/amd64"),
            ("db?name=other", "linux/amd64"),
            ("db", "linux/amd64&name=other"),
            ("db", "windows/amd64"),
        ] {
            assert_eq!(
                create(OsStr::new("unreachable"), name, platform, &Value::Null).await,
                Err(Error::InvalidInput)
            );
        }
    }
}
