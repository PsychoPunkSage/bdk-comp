# BDK Arti-Esplora PoC

A proof of concept for replacing reqwest with async-minreq in rust-esplora-client with Tor integration.

## Project Description

This PoC explores replacing reqwest with async-minreq in rust-esplora-client, focusing on Tor connectivity options. It tests four approaches:

1. **Direct HTTP** using minreq with tokio
2. **SOCKS proxy** attempting to use minreq with Tor
3. **Arti integration** connecting directly to Tor
4. **HTTP-SOCKS bridge** using a local HTTP proxy that forwards to Tor's SOCKS proxy

## Installation

### Prerequisites
- Rust 1.63+
- Tor daemon running on port 9050

```bash
# Install Tor
sudo apt install tor
sudo systemctl start tor

# Build and run
git clone https://github.com/PsychoPunkSage/bdk-comp
cd bdk_arti_esplora
RUST_LOG=debug cargo run
```

## Key Findings

1. **minreq SOCKS limitation**: minreq doesn't support SOCKS proxies (only HTTP CONNECT)
2. **Arti integration works**: Direct Tor integration via Arti is viable
3. **Async compatibility**: minreq works with tokio's `spawn_blocking`
4. **SSL/TLS handling**: Arti's direct connections don't handle SSL/TLS natively, requiring special handling for HTTPS URLs
5. **HTTP-SOCKS bridge viable**: Our lightweight HTTP proxy implementation successfully bridges minreq to Tor's SOCKS proxy

## Alternative Solutions for SOCKS Support

| Alternative                 | Description                                            | Pros                                                                                   | Cons                                                                               |
| --------------------------- | ------------------------------------------------------ | -------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| **Use different client**    | Use libraries like `ureq` or `surf` that support SOCKS | - Native SOCKS support<br>- Simpler implementation                                     | - Doesn't fulfill requirement to use minreq<br>- Additional dependencies           |
| **HTTP-SOCKS bridge**       | Create a local HTTP proxy that connects to SOCKS       | - Keeps minreq<br>- No code changes needed in client<br>- **Implemented successfully** | - Additional component to maintain<br>- Added complexity<br>- Performance overhead |
| **Arti direct integration** | Skip SOCKS and use Arti library directly               | - Cleaner architecture<br>- Better performance<br>- More control over Tor connectivity | - Different programming model<br>- Requires Arti-specific code paths               |

## HTTP-SOCKS Bridge Implementation

The project now includes a working HTTP-SOCKS bridge implementation that allows minreq to connect to Tor:

- **Lightweight HTTP proxy**: Built with Tokio's `TcpListener` without additional dependencies
- **Automatic forwarding**: All HTTP and HTTPS requests are forwarded to Tor's SOCKS proxy (127.0.0.1:9050)
- **CONNECT support**: Properly handles HTTPS tunneling via HTTP CONNECT method
- **SOCKS5 protocol**: Implements proper SOCKS5 handshaking and connection establishment
- **Privacy-preserving**: Domain resolution happens through Tor for better anonymity
- **Easy integration**: Works with minreq's existing HTTP proxy support

### Usage Example:

```rust
// Start the HTTP-SOCKS bridge
let config = BridgeConfig::default(); // Listens on 127.0.0.1:8118
let (bridge_addr, _shutdown_tx) = start_http_socks_bridge(config).await?;

// Use minreq with the HTTP proxy (which forwards to Tor)
let proxy_url = format!("http://{}", bridge_addr);
let response = minreq::get("http://check.torproject.org/api/ip")
    .with_timeout(20)
    .with_proxy(minreq::Proxy::new(proxy_url.as_str())?)
    .send()?;
```

## SSL/TLS Solutions with Arti

| Solution               | Description                                       | Pros                                        | Cons                                                    |
| ---------------------- | ------------------------------------------------- | ------------------------------------------- | ------------------------------------------------------- |
| **Use HTTP only**      | Avoid HTTPS URLs when using Arti                  | - Simplest approach<br>- Works immediately  | - Limited to HTTP sites<br>- Less secure                |
| **TLS adapter**        | Implement TLS handling on top of Arti connections | - Works with HTTPS<br>- Maintains security  | - Complex implementation<br>- Requires crypto libraries |
| **rustls integration** | Integrate rustls with Arti streams                | - Standards-compliant<br>- Good performance | - Additional dependency<br>- Integration complexity     |


## Conclusion

This PoC demonstrates two viable approaches for using minreq with Tor:

1. **Direct Arti integration**: Using the Arti library provides direct access to Tor without SOCKS, but requires special handling for HTTPS URLs.

2. **HTTP-SOCKS bridge**: Our custom implementation successfully bridges minreq (which only supports HTTP proxies) to Tor's SOCKS proxy, allowing it to work with both HTTP and HTTPS URLs through Tor with minimal code changes.

The choice between these approaches depends on specific requirements around code complexity, performance, and additional dependencies. The HTTP-SOCKS bridge provides the simplest integration path if maintaining compatibility with minreq is a priority.