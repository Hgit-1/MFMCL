use crate::{DownloadError, DownloadSettings};
use reqwest::{Client, ClientBuilder, Proxy, Url};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const DEFAULT_TIMEOUT_SEC: u16 = 60;
pub const DEFAULT_RETRIES: u8 = 2;
pub const MAX_TIMEOUT_SEC: u16 = 600;
pub const MAX_RETRIES: u8 = 10;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    /// Disable environment and operating-system proxies, including NO_PROXY discovery.
    #[default]
    None,
    /// Use reqwest's environment/operating-system proxy discovery and bypass rules.
    System,
    /// Use only proxy_url for both HTTP requests and HTTPS CONNECT tunnels.
    Http,
}

pub(crate) fn default_timeout_sec() -> u16 {
    DEFAULT_TIMEOUT_SEC
}

pub(crate) fn default_retries() -> u8 {
    DEFAULT_RETRIES
}

impl DownloadSettings {
    pub fn validate_network(&self) -> Result<(), DownloadError> {
        if !(1..=MAX_TIMEOUT_SEC).contains(&self.timeout_sec) {
            return Err(DownloadError::Configuration(format!(
                "下载超时必须为 1–{MAX_TIMEOUT_SEC} 秒。"
            )));
        }
        if self.retries > MAX_RETRIES {
            return Err(DownloadError::Configuration(format!(
                "下载重试次数必须为 0–{MAX_RETRIES}。"
            )));
        }
        if self.proxy_url.len() > 4096 || self.proxy_url.chars().any(char::is_control) {
            return Err(invalid_proxy());
        }
        // Inactive drafts are retained for the settings editor, never passed to reqwest.
        if self.proxy_mode == ProxyMode::Http {
            self.parsed_proxy_url()?;
        }
        Ok(())
    }

    fn parsed_proxy_url(&self) -> Result<Url, DownloadError> {
        let url = Url::parse(&self.proxy_url).map_err(|_| invalid_proxy())?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || url.port_or_known_default() == Some(0)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || self.proxy_url.trim() != self.proxy_url
            || self.proxy_url.chars().any(char::is_whitespace)
        {
            return Err(invalid_proxy());
        }
        Ok(url)
    }

    /// Apply the same validated transport policy to metadata clients as the downloader.
    /// Callers keep their own redirect and response-validation policies.
    pub fn configure_client(&self, builder: ClientBuilder) -> Result<ClientBuilder, DownloadError> {
        self.validate_network()?;
        let timeout = Duration::from_secs(u64::from(self.timeout_sec));
        let builder = builder
            .connect_timeout(timeout.min(Duration::from_secs(15)))
            .timeout(timeout)
            // The engine owns the exact retry budget, including retries=0.
            .retry(reqwest::retry::never());
        Ok(match self.proxy_mode {
            ProxyMode::None => builder.no_proxy(),
            ProxyMode::System => builder,
            ProxyMode::Http => builder
                .no_proxy()
                .proxy(Proxy::all(self.parsed_proxy_url()?).map_err(|_| invalid_proxy())?),
        })
    }

    /// Metadata has no outer download retry loop. Retry only safe reads in the supplied
    /// host scope; the configured timeout still bounds the complete request/body.
    pub fn configure_metadata_client(
        &self,
        builder: ClientBuilder,
        hosts: &'static [&'static str],
    ) -> Result<ClientBuilder, DownloadError> {
        self.configure_metadata_client_with_post_paths(builder, hosts, &[])
    }

    /// Only explicitly listed read-only JSON lookup endpoints may replay POST bodies.
    pub fn configure_metadata_client_with_post_paths(
        &self,
        builder: ClientBuilder,
        hosts: &'static [&'static str],
        post_paths: &'static [&'static str],
    ) -> Result<ClientBuilder, DownloadError> {
        let policy = reqwest::retry::for_host(MetadataHosts(hosts))
            // The explicit per-request cap is the user's budget, not a traffic percentage.
            .no_budget()
            .max_retries_per_request(u32::from(self.retries))
            .classify_fn(move |request| {
                let safe_read = (matches!(
                    *request.method(),
                    reqwest::Method::GET | reqwest::Method::HEAD
                ) || (request.method() == reqwest::Method::POST
                    && post_paths.contains(&request.uri().path())))
                    && request
                        .uri()
                        .host()
                        .is_some_and(|host| hosts.contains(&host));
                let transient = request.status().is_some_and(|status| {
                    status.is_server_error() || matches!(status.as_u16(), 408 | 429)
                }) || request.error().is_some_and(|error| {
                    error.downcast_ref::<reqwest::Error>().is_some_and(|error| {
                        error.is_connect() || error.is_timeout() || error.is_request()
                    })
                });
                if safe_read && transient {
                    request.retryable()
                } else {
                    request.success()
                }
            });
        Ok(self.configure_client(builder)?.retry(policy))
    }

    pub fn build_client(&self) -> Result<Client, DownloadError> {
        Ok(self
            .configure_client(
                Client::builder().user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION"))),
            )?
            .build()?)
    }
}

struct MetadataHosts(&'static [&'static str]);

impl PartialEq<&str> for MetadataHosts {
    fn eq(&self, host: &&str) -> bool {
        self.0.contains(host)
    }
}

fn invalid_proxy() -> DownloadError {
    DownloadError::Configuration(
        "代理地址必须是有效的 HTTP/HTTPS 代理根地址，且不能包含凭据、路径、查询或片段。".into(),
    )
}
