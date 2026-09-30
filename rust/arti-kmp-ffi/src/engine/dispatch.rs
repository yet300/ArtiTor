//! Shared CONNECT tunnel through the client selected by the owning listener.
use super::*;

/// Minimal SOCKS5 CONNECT handler tunnelling through the Tor client.
/// Ported from the proven bitchat android wrapper (CONNECT only).
pub(super) async fn handle_socks(
    mut stream: tokio::net::TcpStream,
    client: Arc<TorClient<PreferredRuntime>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (host, port) = socks::read_connect_request(&mut stream).await?;

    let tor_stream = match client.connect((host.as_str(), port)).await {
        Ok(s) => s,
        Err(e) => {
            stream
                .write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            return Err(socks::safe_connect_diagnostic(&e).into());
        }
    };
    stream
        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;

    let (mut cr, mut cw) = stream.split();
    let (mut tr, mut tw) = tor_stream.split();
    let c2t = async { tokio::io::copy(&mut cr, &mut tw).await };
    let t2c = async { tokio::io::copy(&mut tr, &mut cw).await };
    tokio::select! {
        _ = c2t => {}
        _ = t2c => {}
    }
    Ok(())
}
