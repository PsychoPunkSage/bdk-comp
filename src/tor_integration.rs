use anyhow::{anyhow, Result};
use arti_client::{config::TorClientConfig, TorClient};
use async_trait::async_trait;
use log::{debug, info};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tor_rtcompat::tokio::TokioNativeTlsRuntime;

/// Create and bootstrap a Tor client
pub async fn create_tor_client() -> Result<TorClient<tor_rtcompat::PreferredRuntime>> {
    let config = TorClientConfig::builder()
        // You can add any config options here
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
    // Note: In a real implementation, you would use a proper HTTP client library
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

/// Custom Transport trait which would be implemented for integrating
/// minreq with Arti directly
#[async_trait]
pub trait TorTransport {
    async fn connect(&self, host: &str, port: u16) -> Result<Box<dyn AsyncStream>>;
}

/// Async stream trait for our transport
#[async_trait]
pub trait AsyncStream: Send + Sync {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
    async fn write(&mut self, buf: &[u8]) -> Result<usize>;
}

/// Implementation of TorTransport for Arti
pub struct ArtiTransport {
    tor_client: TorClient<TokioNativeTlsRuntime>,
}

impl ArtiTransport {
    pub fn new(tor_client: TorClient<TokioNativeTlsRuntime>) -> Self {
        Self { tor_client }
    }
}

#[async_trait]
impl TorTransport for ArtiTransport {
    async fn connect(&self, host: &str, port: u16) -> Result<Box<dyn AsyncStream>> {
        // In a real implementation, this would create a proper Tor circuit
        // and return a stream that implements AsyncStream
        // For this POC, we'll just show the concept

        let url = format!("{}:{}", host, port);
        let stream = self.tor_client.connect(url).await?;

        Ok(Box::new(ArtiStream { stream }))
    }
}

/// Implementation of AsyncStream for Arti's TcpStream
pub struct ArtiStream {
    stream: arti_client::DataStream,
}

#[async_trait]
impl AsyncStream for ArtiStream {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = self.stream.read(buf).await?;
        Ok(n)
    }

    async fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let n = self.stream.write(buf).await?;
        Ok(n)
    }
}

/// A conceptual implementation of how we might integrate minreq with Arti
/// This is not functional code, but demonstrates the approach
pub mod conceptual {
    use super::*;
    use std::sync::Arc;
    use tokio::task;

    /// This would be a custom connector for a hypothetical async version of minreq
    pub struct AsyncTorConnector {
        transport: Arc<dyn TorTransport>,
    }

    impl AsyncTorConnector {
        pub fn new(transport: Arc<dyn TorTransport>) -> Self {
            Self { transport }
        }

        pub async fn get(&self, url: &str) -> Result<Response> {
            // Parse URL to get host and port
            let url = url::Url::parse(url)?;
            let host = url.host_str().unwrap_or("localhost");
            let port = url
                .port()
                .unwrap_or(if url.scheme() == "https" { 443 } else { 80 });

            // Connect via Tor
            let mut stream = self.transport.connect(host, port).await?;

            // Send request
            let request = format!(
                "GET {} HTTP/1.1\r\n\
                 Host: {}\r\n\
                 User-Agent: minreq-tor-poc/0.1.0\r\n\
                 Connection: close\r\n\
                 \r\n",
                url.path(),
                host
            );

            stream.write(request.as_bytes()).await?;

            // Read response
            let mut buffer = vec![0; 4096];
            let mut response_data = Vec::new();

            loop {
                let n = stream.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                response_data.extend_from_slice(&buffer[..n]);
            }

            // Parse response - this would be handled by an HTTP client in practice
            let response = Response {
                status: 200, // Simplified for this example
                body: String::from_utf8(response_data)?,
            };

            Ok(response)
        }
    }

    /// Another approach: wrapping synchronous minreq with tokio tasks
    pub struct TokioMinreqWrapper {
        tor_proxy: String,
    }

    impl TokioMinreqWrapper {
        pub fn new(tor_proxy: String) -> Self {
            Self { tor_proxy }
        }

        pub async fn get(&self, url: &str) -> Result<Response> {
            let proxy = self.tor_proxy.clone();
            let url_str = url.to_string();

            // Use tokio's spawn_blocking to run the synchronous minreq in a separate thread
            let result = task::spawn_blocking(move || {
                let response = minreq::get(&url_str)
                    .with_timeout(10)
                    .with_proxy(minreq::Proxy::new(&proxy)?)
                    .send()?;

                Ok::<_, anyhow::Error>(Response {
                    status: response.status_code as u16,
                    body: response.as_str()?.to_string(),
                })
            })
            .await??;

            Ok(result)
        }
    }

    pub struct Response {
        pub status: u16,
        pub body: String,
    }
}
