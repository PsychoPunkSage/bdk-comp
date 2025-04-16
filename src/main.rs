use anyhow::{anyhow, Result};
use log::{error, info, warn};
use tokio::task;
use url::Url;

mod tor_integration;

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
    // let response = task::spawn_blocking(move || {
    //     minreq::get(TEST_URL)
    //         .with_timeout(30) // Increase timeout for Tor connections
    //         .with_proxy(minreq::Proxy::new_socks5("127.0.0.1", 9050)?)
    //         .send()
    // })
    // .await??;

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
