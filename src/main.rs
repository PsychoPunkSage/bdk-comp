use anyhow::{anyhow, Result};
use log::{error, info, warn};
use tokio::task;
use url::Url;

mod http_socks_bridge;
mod tor_integration;

use http_socks_bridge::{start_http_socks_bridge, BridgeConfig};
use tor_integration::{create_tor_client, fetch_via_arti};

const TEST_URL: &str = "http://check.torproject.org/api/ip";
const ONION_TEST_URL: &str =
    "http://2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid.onion/";
const TOR_SOCKS_PROXY: &str = "socks5://127.0.0.1:9050";

// Add simple logging to help debug any Tor issues
fn setup_logging() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .init();
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_logging();
    info!("Minreq Tor Integration POC");
    info!("==========================");

    // 1. Direct HTTP request with minreq (wrapped in tokio task)
    if let Err(e) = test_direct_http().await {
        error!("Direct HTTP request failed: {}", e);
    }

    // 2. HTTP request via SOCKS proxy (Tor)
    if let Err(e) = test_socks_proxy().await {
        error!("SOCKS proxy request failed: {}", e);
    }

    // 3. HTTP request via Arti Tor client
    if let Err(e) = test_arti_integration().await {
        error!("Arti integration request failed: {}", e);
    }

    // 4. HTTP request via HTTP-SOCKS bridge
    if let Err(e) = test_http_socks_bridge().await {
        error!("HTTP-SOCKS bridge request failed: {}", e);
    }

    Ok(())
}

/// Test direct HTTP request using minreq (wrapped in tokio task for async usage)
async fn test_direct_http() -> Result<()> {
    info!("\n1. Testing direct HTTP request with minreq (async wrapper)...");

    // Since minreq is synchronous, we use tokio's spawn_blocking
    // to avoid blocking the async runtime
    let response =
        task::spawn_blocking(move || minreq::get(TEST_URL).with_timeout(10).send()).await??;

    if response.status_code >= 200 && response.status_code < 300 {
        let body = response.as_str()?;
        info!("✅ Direct HTTP request successful");
        info!("Status: {}", response.status_code);
        info!("Response: {}", body);
        Ok(())
    } else {
        Err(anyhow!(
            "Request failed with status code: {}",
            response.status_code
        ))
    }
}

/// Test HTTP request via SOCKS proxy (Tor)
async fn test_socks_proxy() -> Result<()> {
    info!("\n2. Testing HTTP request via SOCKS proxy (Tor)...");
    info!("   Using proxy: {}", TOR_SOCKS_PROXY);

    // Parse the proxy URL
    let proxy_url = Url::parse(TOR_SOCKS_PROXY)?;

    // Build the request with proxy - also using tokio's spawn_blocking
    // since this is a synchronous operation
    let response = task::spawn_blocking(move || {
        minreq::get(TEST_URL)
            .with_timeout(10)
            .with_proxy(minreq::Proxy::new(proxy_url.as_str())?)
            .send()
    })
    .await??;

    if response.status_code >= 200 && response.status_code < 300 {
        let body = response.as_str()?;
        info!("✅ SOCKS proxy request successful");
        info!("Status: {}", response.status_code);
        info!("Response: {}", body);
        Ok(())
    } else {
        Err(anyhow!(
            "Request failed with status code: {}",
            response.status_code
        ))
    }
}

/// Test HTTP request via Arti Tor client
async fn test_arti_integration() -> Result<()> {
    info!("\n3. Testing HTTP request via Arti Tor client...");

    // Create and bootstrap the Tor client
    let tor_client = create_tor_client().await?;
    info!("   Tor client bootstrapped successfully");

    // Try a regular HTTP URL first
    info!("   Fetching regular HTTP URL via Tor: {}", TEST_URL);
    let response = fetch_via_arti(&tor_client, TEST_URL).await?;
    info!("✅ Regular HTTP request via Tor successful");
    info!("Response: {}", response);

    // Try an onion service
    info!("   Fetching onion service: {}", ONION_TEST_URL);
    match fetch_via_arti(&tor_client, ONION_TEST_URL).await {
        Ok(response) => {
            info!("✅ Onion service request successful");
            info!("Response length: {} bytes", response.len());
            // Print first 100 chars to avoid flooding terminal
            info!(
                "Response (truncated): {}",
                if response.len() > 100 {
                    &response[..100]
                } else {
                    &response
                }
            );
        }
        Err(e) => {
            warn!("❌ Onion service request failed: {}", e);
            warn!(
                "Note: This is expected if Tor is not running or the onion service is unavailable"
            );
        }
    }

    Ok(())
}

/// Test HTTP request via HTTP-SOCKS bridge
/// This demonstrates using minreq with an HTTP proxy that forwards to Tor's SOCKS proxy
async fn test_http_socks_bridge() -> Result<()> {
    info!("\n4. Testing HTTP request via HTTP-SOCKS bridge...");

    // Start the HTTP-SOCKS bridge with default configuration
    // (127.0.0.1:8118 forwarding to 127.0.0.1:9050)
    let config = BridgeConfig::default();
    let (bridge_addr, _shutdown_tx) = start_http_socks_bridge(config).await?;

    info!("   HTTP-SOCKS bridge started on {}", bridge_addr);
    info!("   Using bridge to access: {}", TEST_URL);

    // Create the HTTP proxy URL that minreq will connect to
    let http_proxy_url = format!("http://{}", bridge_addr);

    // Build the request with HTTP proxy - using tokio's spawn_blocking
    // since minreq is synchronous
    let proxy_url = http_proxy_url.clone();
    let response = task::spawn_blocking(move || {
        minreq::get(TEST_URL)
            .with_timeout(20) // Longer timeout for Tor
            .with_proxy(minreq::Proxy::new(proxy_url.as_str())?)
            .send()
    })
    .await??;

    if response.status_code >= 200 && response.status_code < 300 {
        let body = response.as_str()?;
        info!("✅ HTTP-SOCKS bridge request successful");
        info!("Status: {}", response.status_code);
        info!("Response: {}", body);

        // Verify we're going through Tor by checking the response from check.torproject.org
        if body.contains("\"IsTor\":true") {
            info!("✅ Confirmed request went through Tor!");
        } else {
            warn!("⚠️ Request did not go through Tor");
        }

        Ok(())
    } else {
        Err(anyhow!(
            "Request failed with status code: {}",
            response.status_code
        ))
    }

    // Note: We're not shutting down the bridge server here to allow
    // additional requests during the program's lifetime if needed.
    // The server will be shut down when the program exits.
}
