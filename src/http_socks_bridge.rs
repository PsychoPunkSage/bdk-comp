use anyhow::{anyhow, Result};
use log::{debug, error, info};
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use url::Url;

/// Default port for the HTTP-SOCKS bridge
const DEFAULT_PORT: u16 = 8118;

/// Configuration for the HTTP-SOCKS bridge
pub struct BridgeConfig {
    /// Local address to bind the HTTP proxy server
    pub http_bind_addr: SocketAddr,
    /// Address of the SOCKS proxy (Tor)
    pub socks_proxy_addr: String,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            http_bind_addr: format!("127.0.0.1:{}", DEFAULT_PORT).parse().unwrap(),
            socks_proxy_addr: "127.0.0.1:9050".to_string(),
        }
    }
}

/// Starts the HTTP-SOCKS bridge proxy server.
/// Returns the address the server is listening on and a shutdown channel.
pub async fn start_http_socks_bridge(
    config: BridgeConfig,
) -> Result<(SocketAddr, oneshot::Sender<()>)> {
    // Bind to the HTTP proxy address
    let listener = TcpListener::bind(config.http_bind_addr).await?;
    let local_addr = listener.local_addr()?;
    info!(
        "HTTP-SOCKS bridge listening on {}, forwarding to {}",
        local_addr, config.socks_proxy_addr
    );

    // Create a shutdown channel
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

    // Spawn the server task
    tokio::spawn(async move {
        // Clone the SOCKS proxy address to move into the task
        let socks_addr = config.socks_proxy_addr.clone();

        // Accept connections loop
        loop {
            // Use tokio::select to handle either a new connection or a shutdown signal
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, addr)) => {
                            debug!("New connection from {}", addr);
                            // Clone the SOCKS proxy address for each connection handler
                            let socks_proxy = socks_addr.clone();
                            // Spawn a new task to handle this connection
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(stream, &socks_proxy).await {
                                    error!("Error handling connection from {}: {}", addr, e);
                                }
                            });
                        }
                        Err(e) => {
                            error!("Error accepting connection: {}", e);
                        }
                    }
                }
                _ = &mut shutdown_rx => {
                    info!("Shutdown signal received, stopping HTTP-SOCKS bridge");
                    break;
                }
            }
        }
    });

    Ok((local_addr, shutdown_tx))
}

/// Handles a single HTTP proxy connection
async fn handle_connection(mut client_stream: TcpStream, socks_proxy: &str) -> Result<()> {
    // Buffer to read the HTTP request headers
    let mut buffer = vec![0u8; 4096];
    let mut headers = Vec::new();
    let mut header_end_pos = 0;

    // Read the HTTP request headers
    loop {
        let n = client_stream.read(&mut buffer).await?;
        if n == 0 {
            return Err(anyhow!(
                "Client closed connection before sending complete request"
            ));
        }

        headers.extend_from_slice(&buffer[0..n]);

        // Check if we've received the end of the HTTP headers (marked by \r\n\r\n)
        if let Some(pos) = find_header_end(&headers) {
            header_end_pos = pos;
            break;
        }

        // Safety check to prevent buffer from growing too large
        if headers.len() > 32768 {
            return Err(anyhow!("HTTP headers too large"));
        }
    }

    // Convert headers to string for parsing
    let headers_str = String::from_utf8_lossy(&headers[0..header_end_pos]);
    debug!("Received HTTP request:\n{}", headers_str);

    // Extract the request method, URL, and HTTP version
    let request_line = headers_str
        .lines()
        .next()
        .ok_or_else(|| anyhow!("Empty request"))?;
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(anyhow!("Invalid request line: {}", request_line));
    }

    let method = parts[0];
    let url_str = parts[1];
    let http_version = parts[2];

    // Handle CONNECT method differently (used for HTTPS)
    if method == "CONNECT" {
        return handle_connect_method(client_stream, url_str, socks_proxy).await;
    }

    // Parse the target URL
    let url = if url_str.starts_with("http://") || url_str.starts_with("https://") {
        Url::parse(url_str)?
    } else {
        // Handle relative URLs by extracting host from Host header
        let host = extract_host_header(&headers_str)
            .ok_or_else(|| anyhow!("Missing Host header in request"))?;
        let scheme = if host.contains(":443") {
            "https"
        } else {
            "http"
        };
        Url::parse(&format!("{}://{}{}", scheme, host, url_str))?
    };

    // Extract host and port from URL
    let host = url.host_str().ok_or_else(|| anyhow!("No host in URL"))?;
    let port = url
        .port()
        .unwrap_or_else(|| if url.scheme() == "https" { 443 } else { 80 });
    let target = format!("{}:{}", host, port);

    // Connect to the target server via SOCKS proxy
    info!(
        "Connecting to {} via SOCKS proxy at {}",
        target, socks_proxy
    );
    let mut server_stream = match create_socks5_connection(socks_proxy, &target).await {
        Ok(stream) => stream,
        Err(e) => {
            // Send error response back to client
            let error_response = format!(
                "{} 502 Bad Gateway\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nFailed to connect to target server: {}\r\n",
                http_version, e
            );
            client_stream.write_all(error_response.as_bytes()).await?;
            return Err(e);
        }
    };

    // Rewrite the request to make it suitable for the server
    // - Change absolute URL to path
    // - Add/modify headers if needed
    let mut modified_request = Vec::new();
    let path = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    let path_with_query = if let Some(query) = url.query() {
        format!("{}?{}", path, query)
    } else {
        path.to_string()
    };

    // Write request line with the modified path
    modified_request.extend_from_slice(
        format!("{} {} {}\r\n", method, path_with_query, http_version).as_bytes(),
    );

    // Copy headers, except for the Connection header which we'll override
    for line in headers_str.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        if !line.to_lowercase().starts_with("connection:")
            && !line.to_lowercase().starts_with("proxy-")
        {
            modified_request.extend_from_slice(format!("{}\r\n", line).as_bytes());
        }
    }

    // Add our own Connection header
    modified_request.extend_from_slice(b"Connection: close\r\n\r\n");

    // If there's a request body, copy it
    if header_end_pos + 4 < headers.len() {
        modified_request.extend_from_slice(&headers[header_end_pos + 4..]);
    }

    // Send the modified request to the server
    server_stream.write_all(&modified_request).await?;

    // Now relay data bidirectionally until the connection closes
    relay_data(client_stream, server_stream).await?;

    Ok(())
}

/// Handle CONNECT method (used for HTTPS tunneling)
async fn handle_connect_method(
    mut client_stream: TcpStream,
    target: &str,
    socks_proxy: &str,
) -> Result<()> {
    // For CONNECT method, the URL is just "host:port"
    info!("Handling CONNECT request to {}", target);

    // Connect to the target via SOCKS proxy
    let server_stream = match create_socks5_connection(socks_proxy, target).await {
        Ok(stream) => stream,
        Err(e) => {
            // Send error response back to client
            let error_response = "HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n";
            client_stream.write_all(error_response.as_bytes()).await?;
            return Err(e);
        }
    };

    // Send success response to the client
    client_stream
        .write_all(b"HTTP/1.1 200 Connection Established\r\nConnection: close\r\n\r\n")
        .await?;

    // Now relay data bidirectionally until the connection closes
    relay_data(client_stream, server_stream).await?;

    Ok(())
}

/// Create a connection to a target host:port via a SOCKS5 proxy
async fn create_socks5_connection(socks_proxy: &str, target: &str) -> Result<TcpStream> {
    // Parse the proxy address
    let proxy_parts: Vec<&str> = socks_proxy.split(':').collect();
    if proxy_parts.len() != 2 {
        return Err(anyhow!("Invalid SOCKS proxy address: {}", socks_proxy));
    }
    let proxy_host = proxy_parts[0];
    let proxy_port = proxy_parts[1].parse::<u16>()?;

    // Connect to the SOCKS proxy
    let mut proxy_stream = TcpStream::connect(format!("{}:{}", proxy_host, proxy_port)).await?;

    // Parse target
    let target_parts: Vec<&str> = target.split(':').collect();
    if target_parts.len() != 2 {
        return Err(anyhow!("Invalid target address: {}", target));
    }
    let target_host = target_parts[0];
    let target_port = target_parts[1].parse::<u16>()?;

    // SOCKS5 handshake (no authentication)
    // Send authentication method selection message
    proxy_stream.write_all(&[0x05, 0x01, 0x00]).await?;

    // Read the server's response
    let mut response = [0u8; 2];
    proxy_stream.read_exact(&mut response).await?;

    if response[0] != 0x05 || response[1] != 0x00 {
        return Err(anyhow!(
            "SOCKS5 handshake failed: {:02x} {:02x}",
            response[0],
            response[1]
        ));
    }

    // Send connection request
    let mut request = Vec::new();
    request.push(0x05); // SOCKS version
    request.push(0x01); // TCP connection command
    request.push(0x00); // Reserved

    // Address type and destination
    if target_host.parse::<std::net::Ipv4Addr>().is_ok() {
        // IPv4 address
        request.push(0x01); // IPv4 address type
        for octet in target_host.parse::<std::net::Ipv4Addr>()?.octets() {
            request.push(octet);
        }
    } else if target_host.parse::<std::net::Ipv6Addr>().is_ok() {
        // IPv6 address
        request.push(0x04); // IPv6 address type
        for segment in target_host.parse::<std::net::Ipv6Addr>()?.segments() {
            request.push((segment >> 8) as u8);
            request.push((segment & 0xff) as u8);
        }
    } else {
        // Domain name
        request.push(0x03); // Domain name address type
        let host_bytes = target_host.as_bytes();
        request.push(host_bytes.len() as u8); // Domain name length
        request.extend_from_slice(host_bytes); // Domain name
    }

    // Port (big endian)
    request.push((target_port >> 8) as u8);
    request.push((target_port & 0xff) as u8);

    // Send the connection request
    proxy_stream.write_all(&request).await?;

    // Read the server's response (at least 10 bytes for IPv4)
    let mut response = [0u8; 10];
    proxy_stream.read_exact(&mut response).await?;

    if response[0] != 0x05 || response[1] != 0x00 {
        return Err(anyhow!(
            "SOCKS5 connection request failed: {:02x} {:02x}",
            response[0],
            response[1]
        ));
    }

    // If the response contains an IPv6 address, we need to read 12 more bytes
    if response[3] == 0x04 {
        let mut ipv6_tail = [0u8; 12];
        proxy_stream.read_exact(&mut ipv6_tail).await?;
    } else if response[3] == 0x03 {
        // Domain name response, need to read the domain length + domain + port
        let domain_len = response[4] as usize;
        let mut domain_data = vec![0u8; domain_len + 2 - 1]; // -1 because we already read the first byte
        proxy_stream.read_exact(&mut domain_data).await?;
    }

    Ok(proxy_stream)
}

/// Find the end of HTTP headers (marked by \r\n\r\n)
fn find_header_end(buf: &[u8]) -> Option<usize> {
    for i in 0..buf.len() - 3 {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' && buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
            return Some(i + 4);
        }
    }
    None
}

/// Extract the Host header from HTTP headers
fn extract_host_header(headers: &str) -> Option<String> {
    for line in headers.lines() {
        if line.to_lowercase().starts_with("host:") {
            return Some(line["host:".len()..].trim().to_string());
        }
    }
    None
}

/// Relay data bidirectionally between client and server until one of them closes the connection
async fn relay_data(mut client: TcpStream, mut server: TcpStream) -> Result<()> {
    // Split the streams into read and write halves
    let (mut client_reader, mut client_writer) = client.split();
    let (mut server_reader, mut server_writer) = server.split();

    // Create separate buffers for each direction of data flow
    let mut client_buffer = vec![0u8; 8192];
    let mut server_buffer = vec![0u8; 8192];

    // Client to server task
    let client_to_server = async {
        loop {
            match client_reader.read(&mut client_buffer).await {
                Ok(0) => break, // Client closed connection
                Ok(n) => {
                    if server_writer.write_all(&client_buffer[0..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        // Shutdown the write half of the server connection
        let _ = server_writer.shutdown().await;
    };

    // Server to client task
    let server_to_client = async {
        loop {
            match server_reader.read(&mut server_buffer).await {
                Ok(0) => break, // Server closed connection
                Ok(n) => {
                    if client_writer.write_all(&server_buffer[0..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        // Shutdown the write half of the client connection
        let _ = client_writer.shutdown().await;
    };

    // Run both tasks concurrently until one of them completes
    tokio::select! {
        _ = client_to_server => {},
        _ = server_to_client => {},
    }

    Ok(())
}
