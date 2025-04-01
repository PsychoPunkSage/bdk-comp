use anyhow::{Context, Result};
use log::{info, warn};
use std::time::Instant;
mod arti_transport;

// URLs for testing
const TEST_URL: &str = "https://check.torproject.org/api/ip";
const TEST_ONION_URL: &str =
    "http://2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid.onion/";

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logger
    env_logger::init();
    info!("Starting Tor integration PoC");

    // Run all test methods, ignoring failures so we can try each approach
    let methods = [
        ("Direct Connection", direct_request),
        (
            "SOCKS proxy (require running Tor daemon)",
            socks_proxy_request,
        ),
        ("Arti direct integration", arti_direct_request),
    ];

    for (name, method) in methods {
        info!("Testing method: {}", name);
        let start = Instant::now();
        match method().await {
            Ok(response) => {
                let duration = start.elapsed();
                info!("✅ Success ({:?}): {}", duration, response);
            }
            Err(e) => {
                warn!("❌ Failed: {:#}", e);
            }
        }
        info!("------------------------");
    }

    Ok(())
}

/// Standard HTTP request without any proxying
async fn direct_request() -> Result<String> {
    info!("Making direct HTTP request");

    let response = async_minreq::get(TEST_URL)
        .send()
        .await
        .context("Failed to send direct request")?;

    if !response.status_code.is_success() {
        anyhow::bail!("HTTP error: {}", response.status_code);
    }

    Ok(response.as_str()?.to_string())
}

/// HTTP request via SOCKS proxy (requires running Tor daemon)
async fn socks_proxy_request() -> Result<String> {
    info!("Making request via SOCKS proxy");

    // Tor default SOCKS port is 9050
    let response = async_minreq::get(TEST_URL)
        .with_proxy("socks5://127.0.0.1:9050")
        .context("Failed to configure SOCKS proxy")?
        .send()
        .await
        .context("Failed to send request via SOCKS proxy")?;

    if !response.status_code.is_success() {
        anyhow::bail!("HTTP error: {}", response.status_code);
    }

    Ok(response.as_str()?.to_string())
}

/// Attempt to use arti directly without a SOCKS proxy
async fn arti_direct_request() -> Result<String> {
    info!("Making request via direct arti integration");

    let tor_client = arti_transport::create_tor_client().await?;

    // This is a proof of concept for the approach that would need to be implemented
    // The actual integration would require creating a custom connector for async-minreq
    let response = arti_transport::fetch_via_tor(&tor_client, TEST_URL).await?;

    Ok(response)
}
