use anyhow::{anyhow, Result};
use arti_client::{config::TorClientConfig, TorClient};
use log::{debug, info};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Create and bootstrap a Tor client
pub async fn create_tor_client() -> Result<TorClient<tor_rtcompat::PreferredRuntime>> {
    let config = TorClientConfig::builder()
        // I can add any config options here
        .build()
        .expect("Failed to build config");

    // Create the Tor client with the configuration
    info!("Creating and bootstrapping Tor client...");
    let tor_client = TorClient::create_bootstrapped(config).await?;
    info!("Tor client successfully bootstrapped!");

    Ok(tor_client)
}

/// Fetch content via Arti Tor client
pub async fn fetch_via_arti(
    tor_client: &TorClient<tor_rtcompat::PreferredRuntime>,
    url: &str,
) -> Result<String> {
    debug!("Fetching URL via Arti: {}", url);

    // Parse the URL
    let parsed_url = url::Url::parse(url)?;
    // let host = parsed_url.host_str().unwrap_or("unknown").to_string();
    let host = parsed_url
        .host_str()
        .ok_or_else(|| anyhow!("No host in URL"))?;
    let port = parsed_url.port().unwrap_or_else(|| {
        if parsed_url.scheme() == "https" {
            443
        } else {
            80
        }
    });
    // Format address for Arti in the required format: hostname:port
    let addr = format!("{}:{}", host, port);
    info!("Connecting to Tor address: {}", addr);

    // Create a Tor connection to the target
    let mut stream = tor_client.connect(&addr).await?;
    debug!("Connection established to target");

    // Format path and query
    let path = if parsed_url.path().is_empty() {
        "/"
    } else {
        parsed_url.path()
    };
    let request_path = if let Some(query) = parsed_url.query() {
        format!("{}?{}", path, query)
    } else {
        path.to_string()
    };

    // Craft a simple HTTP request
    // Note: In a real implementation, I would use a proper HTTP client library
    // This is just for demonstration purposes
    let request = format!(
        "GET {} HTTP/1.1\r\n\
         Host: {}\r\n\
         User-Agent: minreq-tor-poc/0.1.0\r\n\
         Accept: */*\r\n\
         Connection: close\r\n\
         \r\n",
        request_path, host
    );

    // Send the request
    info!("Sending request:\n{}", request);
    stream.write_all(request.as_bytes()).await?;
    info!("Request sent, waiting for response...");

    // Read with a much longer timeout
    let mut response = Vec::new();
    let mut buffer = vec![0; 4096];
    let timeout = Duration::from_secs(60); // Increased timeout

    let read_future = async {
        loop {
            match stream.read(&mut buffer).await {
                Ok(0) => break, // End of stream
                Ok(n) => {
                    response.extend_from_slice(&buffer[..n]);
                    info!("Read {} bytes from stream", n);
                }
                Err(e) => return Err(anyhow!("Error reading from stream: {}", e)),
            }
        }
        Ok(())
    };

    match tokio::time::timeout(timeout, read_future).await {
        Ok(result) => result?,
        Err(_) => return Err(anyhow!("Timeout while reading response")),
    }

    // Convert the response bytes to a String
    let response_string = String::from_utf8(response)
        .map_err(|e| anyhow!("Failed to parse response as UTF-8: {}", e))?;
    Ok(response_string)
}
