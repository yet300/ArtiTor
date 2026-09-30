//! CONNECT-only SOCKS5 framing, shared by root and isolation listeners.
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SocksRequestError {
    UnsupportedCommand,
    TruncatedIpv6,
    UnsupportedAtyp,
    Malformed,
}

fn valid_domain(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

pub(crate) fn parse_socks_request(
    buf: &[u8],
    n: usize,
) -> Result<(String, u16), SocksRequestError> {
    let buf = buf.get(..n).ok_or(SocksRequestError::Malformed)?;
    if buf.len() < 4 || buf[0] != 5 || buf[2] != 0 {
        return Err(SocksRequestError::Malformed);
    }
    if buf[1] != 1 {
        return Err(SocksRequestError::UnsupportedCommand);
    }
    match buf[3] {
        1 => {
            if buf.len() < 10 {
                return Err(SocksRequestError::Malformed);
            }
            Ok((
                format!("{}.{}.{}.{}", buf[4], buf[5], buf[6], buf[7]),
                u16::from_be_bytes([buf[8], buf[9]]),
            ))
        }
        3 => {
            let len = *buf.get(4).ok_or(SocksRequestError::Malformed)? as usize;
            let bytes = buf.get(5..5 + len).ok_or(SocksRequestError::Malformed)?;
            let host = std::str::from_utf8(bytes).map_err(|_| SocksRequestError::Malformed)?;
            if !valid_domain(host) || buf.len() < 7 + len {
                return Err(SocksRequestError::Malformed);
            }
            Ok((
                host.to_owned(),
                u16::from_be_bytes([buf[5 + len], buf[6 + len]]),
            ))
        }
        4 => {
            if buf.len() < 22 {
                return Err(SocksRequestError::TruncatedIpv6);
            }
            let segments: Vec<String> = buf[4..20]
                .chunks_exact(2)
                .map(|s| format!("{:x}", u16::from_be_bytes([s[0], s[1]])))
                .collect();
            Ok((segments.join(":"), u16::from_be_bytes([buf[20], buf[21]])))
        }
        _ => Err(SocksRequestError::UnsupportedAtyp),
    }
}

async fn reject<S: AsyncWrite + Unpin>(stream: &mut S, code: u8) -> io::Result<()> {
    stream.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid SOCKS frame")
}

/// Consume exactly the greeting and CONNECT request; leave pipelined payload unread.
/// Hostnames are passed through to Tor without local DNS resolution.
pub(crate) async fn read_connect_request<S>(stream: &mut S) -> io::Result<(String, u16)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut greeting = [0; 2];
    stream.read_exact(&mut greeting).await?;
    let mut methods = vec![0; greeting[1] as usize];
    stream.read_exact(&mut methods).await?;
    if greeting[0] != 5 || !methods.contains(&0) {
        stream.write_all(&[5, 0xff]).await?;
        return Err(invalid());
    }
    stream.write_all(&[5, 0]).await?;
    let mut header = [0; 4];
    stream.read_exact(&mut header).await?;
    let rejection = if header[0] != 5 || header[2] != 0 {
        Some(1)
    } else if header[1] != 1 {
        Some(7)
    } else if ![1, 3, 4].contains(&header[3]) {
        Some(8)
    } else {
        None
    };
    if let Some(code) = rejection {
        reject(stream, code).await?;
        return Err(invalid());
    }
    let mut frame = header.to_vec();
    let length = match header[3] {
        1 => 6,
        4 => 18,
        3 => {
            let len = stream.read_u8().await?;
            frame.push(len);
            usize::from(len) + 2
        }
        _ => unreachable!(),
    };
    let offset = frame.len();
    frame.resize(offset + length, 0);
    if let Err(error) = stream.read_exact(&mut frame[offset..]).await {
        reject(stream, 1).await?;
        return Err(error);
    }
    match parse_socks_request(&frame, frame.len()) {
        Ok(target) => Ok(target),
        Err(_) => {
            reject(stream, 1).await?;
            Err(invalid())
        }
    }
}

/// Only the upstream categorical enum is included, never the error's target-bearing Display.
pub(crate) fn safe_connect_diagnostic(error: &arti_client::Error) -> String {
    use arti_client::HasKind;
    format!(
        "SOCKS CONNECT failed (code=5, arti_kind={:?})",
        error.kind()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[test]
    fn safe_diagnostic_omits_target_and_upstream_display() {
        use arti_client::IntoTorAddr;
        let sensitive_target = "sensitive target marker.example";
        // This is the same upstream address-validation error that connect()
        // produces before any bootstrap or network access is attempted.
        let error: arti_client::Error = (sensitive_target, 80)
            .into_tor_addr()
            .expect_err("spaces are invalid in upstream Tor hostnames")
            .into();
        let diagnostic = safe_connect_diagnostic(&error);
        assert_eq!(
            diagnostic,
            "SOCKS CONNECT failed (code=5, arti_kind=InvalidStreamTarget)"
        );
        assert!(!diagnostic.contains(sensitive_target));
        assert!(!diagnostic.contains(&error.to_string()));
        assert!(!diagnostic.contains("Rejecting hostname"));
    }

    async fn exchange(greeting: &[u8], request: &[u8]) -> (io::Result<(String, u16)>, Vec<u8>) {
        let (mut client, mut server) = duplex(1);
        let server = tokio::spawn(async move { read_connect_request(&mut server).await });
        // Capacity 1 makes every byte cross a separate AsyncRead boundary deterministically.
        for byte in greeting {
            client.write_all(&[*byte]).await.unwrap();
        }
        let mut response = [0; 2];
        client.read_exact(&mut response).await.unwrap();
        if response[1] == 0xff {
            return (server.await.unwrap(), response.to_vec());
        }
        // Rejections can happen at the header; concurrent writing avoids duplex deadlocks.
        let (mut reader, mut writer) = tokio::io::split(client);
        let request = request.to_vec();
        let write = tokio::spawn(async move {
            for byte in request {
                if writer.write_all(&[byte]).await.is_err() {
                    break;
                }
            }
            let _ = writer.shutdown().await;
        });
        let mut replies = response.to_vec();
        reader.read_to_end(&mut replies).await.unwrap();
        write.await.unwrap();
        (server.await.unwrap(), replies)
    }

    #[tokio::test]
    async fn one_byte_greeting_fragmented_header_body_and_one_char_host() {
        let (target, response) = exchange(&[5, 2, 2, 0], &[5, 1, 0, 3, 1, b'a', 0, 80]).await;
        assert_eq!(target.unwrap(), ("a".into(), 80));
        assert_eq!(response, [5, 0]);
    }

    #[tokio::test]
    async fn fragmented_ipv4_and_ipv6() {
        assert_eq!(
            exchange(&[5, 1, 0], &[5, 1, 0, 1, 127, 0, 0, 1, 1, 187])
                .await
                .0
                .unwrap(),
            ("127.0.0.1".into(), 443)
        );
        let mut ipv6 = vec![5, 1, 0, 4];
        ipv6.extend_from_slice(&[0; 15]);
        ipv6.extend_from_slice(&[1, 0, 80]);
        assert_eq!(
            exchange(&[5, 1, 0], &ipv6).await.0.unwrap(),
            ("0:0:0:0:0:0:0:1".into(), 80)
        );
    }

    #[tokio::test]
    async fn greeting_rejects_auth_only_bad_version_and_no_methods() {
        for greeting in [&[5, 1, 2][..], &[4, 1, 0], &[5, 0]] {
            let (result, response) = exchange(greeting, &[]).await;
            assert!(result.is_err());
            assert_eq!(response, [5, 0xff]);
        }
    }

    #[tokio::test]
    async fn rejects_request_version_reserved_bind_udp_and_unknown_atyp() {
        for (header, code) in [
            ([4, 1, 0, 1], 1),
            ([5, 1, 1, 1], 1),
            ([5, 2, 0, 1], 7),
            ([5, 3, 0, 1], 7),
            ([5, 1, 0, 9], 8),
        ] {
            let (result, response) = exchange(&[5, 1, 0], &header).await;
            assert!(result.is_err());
            assert_eq!(&response[2..], &[5, code, 0, 1, 0, 0, 0, 0, 0, 0]);
        }
    }

    #[tokio::test]
    async fn rejects_empty_invalid_utf8_and_malformed_hostname() {
        for host in [&[][..], &[0xff], b"a b", b"-a", b"a..b"] {
            let mut request = vec![5, 1, 0, 3, host.len() as u8];
            request.extend_from_slice(host);
            request.extend_from_slice(&[0, 80]);
            let (result, response) = exchange(&[5, 1, 0], &request).await;
            assert!(result.is_err());
            assert_eq!(response[3], 1);
        }
    }

    #[tokio::test]
    async fn exact_frames_preserve_coalesced_application_payload() {
        let (mut client, mut server) = duplex(128);
        client
            .write_all(&[5, 1, 0, 5, 1, 0, 3, 1, b'a', 0, 80, b'G', b'E', b'T'])
            .await
            .unwrap();
        assert_eq!(
            read_connect_request(&mut server).await.unwrap(),
            ("a".into(), 80)
        );
        let mut payload = [0; 3];
        server.read_exact(&mut payload).await.unwrap();
        assert_eq!(&payload, b"GET");
    }
}
