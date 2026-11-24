//! One retained TLS/ALPN-verified HTTP/2 connection. No redirect, fallback, or implicit resend.
use crate::r3::{self, Body, Branch, Catalog, Detail, History, Id, Intent, Lookup, Node};
use crate::{Result, hex, invalid};
use base64::Engine;
use bytes::Bytes;
use h2::{RecvStream, SendStream, client::SendRequest};
use http::{HeaderMap, HeaderValue, Request, Response, StatusCode, Uri, Version, header};
use rustls::{ClientConfig, RootCertStore, pki_types::ServerName};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use std::{fs::File, future::poll_fn, io::Write, net::Ipv6Addr, sync::Arc, time::Duration};
use tokio::{io::AsyncReadExt, net::TcpStream, task::JoinHandle, time::timeout};
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

#[cfg(target_os = "linux")]
mod receipts;
#[cfg(target_os = "linux")]
pub use receipts::ReceiptSession;

const IDLE: Duration = Duration::from_secs(30);
const CHUNK: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Principal {
    origin: String,
    account: String,
    trust: String,
}

impl Principal {
    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn account(&self) -> &str {
        &self.account
    }
}

struct H2Channel {
    principal: Principal,
    sender: SendRequest<Bytes>,
    driver: JoinHandle<()>,
}

impl Drop for H2Channel {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

type Origin = (String, u16, String);

impl H2Channel {
    async fn connect(
        (host, port, origin): Origin,
        account: &str,
        bound: Option<&Principal>,
    ) -> Result<Self> {
        if std::env::var_os("SSL_CERT_FILE").is_some() || std::env::var_os("SSL_CERT_DIR").is_some()
        {
            return Err(invalid(
                "ambient SSL_CERT_FILE/SSL_CERT_DIR overrides are not admitted; enroll the native trust store explicitly",
            ));
        }
        let root = rustls_native_certs::load_native_certs();
        let mut proof: Vec<_> = root
            .certs
            .iter()
            .map(|cert| Sha256::digest(cert.as_ref()))
            .collect();
        proof.sort_unstable();
        proof.dedup();
        let mut trust = Sha256::new();
        trust.update(b"zxs-core/native-roots/v1\x00");
        for digest in proof {
            trust.update(digest);
        }
        let principal = Principal {
            origin,
            account: account.to_owned(),
            trust: hex(&trust.finalize()),
        };
        require_original_principal(bound, &principal)?;
        let mut store = RootCertStore::empty();
        if store.add_parsable_certificates(root.certs).0 == 0 {
            return Err(invalid("no usable native TLS trust roots"));
        }
        let mut config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()?
                .with_root_certificates(store)
                .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];
        config.enable_early_data = false;
        let sni = ServerName::try_from(host.clone())?;
        let stream = timeout(IDLE, TcpStream::connect((host.as_str(), port))).await??;
        let tls = timeout(
            IDLE,
            TlsConnector::from(Arc::new(config)).connect(sni, stream),
        )
        .await??;
        if tls.get_ref().1.alpn_protocol() != Some(b"h2") {
            return Err(invalid(
                "the same authenticated TLS channel did not negotiate HTTP/2",
            ));
        }
        // No authorization header is constructed before the selected TLS and ALPN checks.
        let (sender, drive) = timeout(IDLE, h2::client::handshake(tls)).await??;
        let driver = tokio::spawn(async move {
            let _ = drive.await;
        });
        Ok(Self {
            principal,
            sender,
            driver,
        })
    }

    async fn request(
        &self,
        mut request: Request<()>,
        auth: &HeaderValue,
        upload: Option<(File, &Body)>,
        before_send: impl FnOnce() -> Result<()>,
    ) -> Result<Response<RecvStream>> {
        let target = request
            .uri()
            .path_and_query()
            .ok_or_else(|| invalid("missing original R3 path"))?;
        if !target.as_str().starts_with("/zxs/v1/")
            || request.uri().scheme().is_some()
            || request.uri().authority().is_some()
        {
            return Err(invalid("refusing an off-origin request"));
        }
        *request.uri_mut() = format!("{}{}", self.principal.origin, target.as_str()).parse()?;
        *request.version_mut() = Version::HTTP_2;
        request
            .headers_mut()
            .insert(header::AUTHORIZATION, auth.clone());
        let mut available = timeout(IDLE, self.sender.clone().ready()).await??;
        before_send()?;
        let (head, mut tx) = available.send_request(request, upload.is_none())?;
        if let Some((file, expected)) = upload {
            send_complete(&mut tx, file, expected).await?;
        }
        Ok(timeout(IDLE, head).await??)
    }
}

fn require_original_principal(bound: Option<&Principal>, observed: &Principal) -> Result<()> {
    if bound.is_some_and(|original| original != observed) {
        return Err(invalid(
            "the original client authority or native trust changed",
        ));
    }
    Ok(())
}

fn validate_account(account: &str) -> Result<()> {
    if account.is_empty()
        || account.len() > 64
        || !account
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        || !account
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
    {
        return Err(invalid("invalid Basic credential for this R3 endpoint"));
    }
    Ok(())
}

fn validate_password(password: &str) -> Result<()> {
    if password.is_empty() || password.len() > 1024 || password.bytes().any(|b| b < 32 || b == 127)
    {
        return Err(invalid("invalid Basic credential for this R3 endpoint"));
    }
    Ok(())
}

fn basic_header(account: &str, password: &str) -> Result<HeaderValue> {
    let mut secret = Zeroizing::new(account.as_bytes().to_vec());
    secret.push(b':');
    secret.extend_from_slice(password.as_bytes());
    let encoded = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(&secret[..]));
    let mut basic = Zeroizing::new(String::from("Basic "));
    basic.push_str(&encoded);
    let mut auth = HeaderValue::from_str(&basic)?;
    auth.set_sensitive(true);
    Ok(auth)
}

/// Legacy Basic R3 transport. Its endpoint capabilities do not change.
pub struct Session {
    channel: H2Channel,
    auth: HeaderValue,
}

impl Session {
    pub async fn connect(
        spelling: &str,
        account: &str,
        password: Zeroizing<String>,
    ) -> Result<Self> {
        let original = approved_origin(spelling)?;
        validate_account(account)?;
        validate_password(&password)?;
        let channel = H2Channel::connect(original, account, None).await?;
        let auth = basic_header(account, &password)?;
        Ok(Self { channel, auth })
    }

    pub fn principal(&self) -> &Principal {
        &self.channel.principal
    }

    async fn request(
        &self,
        request: Request<()>,
        upload: Option<(File, &Body)>,
        before_send: impl FnOnce() -> Result<()>,
    ) -> Result<Response<RecvStream>> {
        self.channel
            .request(request, &self.auth, upload, before_send)
            .await
    }

    async fn json<T: DeserializeOwned>(&self, request: Request<()>) -> Result<T> {
        let response = self.request(request, None, || Ok(())).await?;
        if response.status() != StatusCode::OK {
            return Err(invalid("R3 metadata request not confirmed"));
        }
        Ok(serde_json::from_slice(&collect_json(response).await?)?)
    }

    pub async fn catalog(&self, after: Option<&Id>) -> Result<Catalog> {
        let mut request = r3::json_request(r3::ROOT.to_string())?;
        request
            .headers_mut()
            .insert("zxs-view", HeaderValue::from_static("compact"));
        if let Some(after) = after {
            request
                .headers_mut()
                .insert("zxs-after", HeaderValue::from_str(after.as_str())?);
        }
        let page: Catalog = self.json(request).await?;
        if page.items.len() > 256
            || page
                .items
                .iter()
                .any(|row| row.sequence == 0 || row.sequence == u64::MAX)
            || page
                .items
                .windows(2)
                .any(|w| w[0].container_id >= w[1].container_id)
            || page
                .items
                .first()
                .is_some_and(|first| after.is_some_and(|a| a >= &first.container_id))
            || page.next_after.as_ref()
                != if page.items.len() == 256 {
                    page.items.last().map(|r| &r.container_id)
                } else {
                    None
                }
        {
            return Err(invalid("invalid R3 compact catalog continuation"));
        }
        Ok(page)
    }

    pub async fn detail(&self, container: &Id) -> Result<Detail> {
        let mut request = r3::json_request(format!("{}/{container}", r3::ROOT))?;
        request
            .headers_mut()
            .insert("zxs-detail", HeaderValue::from_static("paged"));
        let detail: Detail = self.json(request).await?;
        if &detail.container_id != container
            || detail.sequence == 0
            || detail.sequence == u64::MAX
            || detail.heads.len() > 256
            || detail.heads.windows(2).any(|w| w[0].branch >= w[1].branch)
            || detail.next_after.as_ref()
                != if detail.heads.len() == 256 {
                    detail.heads.last().map(|r| &r.branch)
                } else {
                    None
                }
        {
            return Err(invalid("invalid R3 container detail"));
        }
        Ok(detail)
    }

    pub async fn selected_head(&self, container: &Id, branch: &Branch) -> Result<(Detail, Node)> {
        let detail = self.detail(container).await?;
        let pinned = detail
            .heads
            .iter()
            .find(|row| &row.branch == branch)
            .ok_or_else(|| invalid("branch not found in first R3 detail page"))?;
        let head = &pinned.head;
        let mut request =
            r3::json_request(format!("{}/{container}/branches/{branch}/nodes", r3::ROOT))?;
        request
            .headers_mut()
            .insert("zxs-history-head", HeaderValue::from_str(head.as_str())?);
        let history: History = self.json(request).await?;
        let node = history
            .nodes
            .first()
            .ok_or_else(|| invalid("selected R3 head was not returned"))?;
        if &history.head != head
            || &node.node_id != head
            || node.sequence == 0
            || node.sequence > detail.sequence
            || node.parent_node.as_ref() == Some(head)
            || self.detail(container).await? != detail
        {
            return Err(invalid("the original R3 head changed during selection"));
        }
        Ok((detail, node.clone()))
    }

    pub async fn download(
        &self,
        container: &Id,
        node: &Node,
        destination: &mut impl Write,
    ) -> Result<()> {
        let request = Request::builder()
            .uri(format!(
                "{}/{container}/nodes/{}/data",
                r3::ROOT,
                node.node_id
            ))
            .header("accept", "application/octet-stream")
            .header("accept-encoding", "identity")
            .body(())?;
        let response = self.request(request, None, || Ok(())).await?;
        if response.status() != StatusCode::OK
            || extent(response.headers(), "application/octet-stream")? != node.length
            || single(response.headers(), "zxs-sha256").map(HeaderValue::as_bytes)
                != Some(node.sha256.as_str().as_bytes())
        {
            return Err(invalid(
                "R3 node response does not prove selected length/digest; no Range used here",
            ));
        }
        let mut stream = response.into_body();
        let mut remaining = node.length;
        let mut sha = Sha256::new();
        while let Some(bytes) = timeout(IDLE, stream.data()).await? {
            let bytes = bytes?;
            if bytes.len() as u64 > remaining {
                return Err(invalid("R3 body exceeded its exact length"));
            }
            destination.write_all(&bytes)?;
            sha.update(&bytes);
            remaining -= bytes.len() as u64;
            stream.flow_control().release_capacity(bytes.len())?;
        }
        if remaining != 0
            || hex(&sha.finalize()) != node.sha256.as_str()
            || has_trailers(&mut stream).await?
        {
            return Err(invalid("R3 full-node EOF, SHA-256 or trailers failed"));
        }
        Ok(())
    }

    pub async fn lookup(&self, intent: &Intent) -> Lookup {
        match intent.lookup() {
            Ok(request) => self.resolve(intent, request, None, true, || Ok(())).await,
            Err(_) => Lookup::Unknown,
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) async fn save_exact(
        &self,
        intent: &Intent,
        file: File,
        before_send: impl FnOnce() -> Result<()>,
    ) -> Result<Lookup> {
        let Ok(request) = intent.save() else {
            return Ok(Lookup::Unknown);
        };
        let mut refused = None;
        let reply = self
            .resolve(intent, request, Some((file, &intent.body)), false, || {
                if let Err(error) = before_send() {
                    refused = Some(error);
                    return Err(invalid("local authorization refused before sending"));
                }
                Ok(())
            })
            .await;
        // An observed local refusal sent no POST. Do not treat it as a wire
        // uncertainty or write another local phase on the detached custody.
        refused.map_or(Ok(reply), Err)
    }

    async fn resolve(
        &self,
        intent: &Intent,
        request: Request<()>,
        upload: Option<(File, &Body)>,
        readback: bool,
        before_send: impl FnOnce() -> Result<()>,
    ) -> Lookup {
        let Ok(response) = self.request(request, upload, before_send).await else {
            return Lookup::Unknown;
        };
        decode_r3_reply(intent, response, readback).await
    }
}

async fn decode_r3_reply(
    intent: &Intent,
    response: Response<RecvStream>,
    readback: bool,
) -> Lookup {
    let status = response.status();
    if status == StatusCode::CONFLICT {
        return Lookup::Conflict;
    }
    if status != StatusCode::OK && !(status == StatusCode::NOT_FOUND && readback) {
        return Lookup::Unknown;
    }
    match collect_json(response).await {
        Ok(bytes) => r3::outcome(intent, status, &bytes, readback),
        Err(_) => Lookup::Unknown,
    }
}

fn approved_origin(spelling: &str) -> Result<Origin> {
    // Authority::port_u16 returns None for both "no explicit port" and certain invalid
    // suffixes. Never send credentials to the default port for the latter.
    if !spelling.starts_with("https://")
        || spelling
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        || spelling.contains(['#', '?', '@'])
    {
        return Err(invalid(
            "use an explicit HTTPS origin, not an account or URL path",
        ));
    }
    let parsed: Uri = spelling.parse()?;
    let auth = parsed
        .authority()
        .ok_or_else(|| invalid("supply a single HTTPS origin"))?;
    if parsed.scheme_str() != Some("https") || parsed.path() != "/" || parsed.query().is_some() {
        return Err(invalid(
            "use an explicit HTTPS origin without path or query",
        ));
    }
    let token = auth.host();
    let suffix = auth
        .as_str()
        .strip_prefix(token)
        .ok_or_else(|| invalid("invalid exact HTTPS authority"))?;
    let port = match suffix {
        "" => 443,
        raw => {
            let raw = raw
                .strip_prefix(':')
                .ok_or_else(|| invalid("invalid explicit HTTPS port"))?;
            if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid("invalid explicit HTTPS port"));
            }
            raw.parse::<u16>()
                .ok()
                .filter(|p| *p != 0)
                .ok_or_else(|| invalid("explicit HTTPS port out of range"))?
        }
    };
    let (host, printed) =
        if let Some(ip) = token.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            let ip: Ipv6Addr = ip.parse()?;
            (ip.to_string(), format!("[{ip}]"))
        } else {
            if token.is_empty()
                || !token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
            {
                return Err(invalid("invalid exact HTTPS hostname"));
            }
            let name = token.to_ascii_lowercase();
            (name.clone(), name)
        };
    Ok((host, port, format!("https://{printed}:{port}")))
}

fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a HeaderValue> {
    let found = headers.get_all(name);
    let mut entries = found.iter();
    let value = entries.next()?;
    entries.next().is_none().then_some(value)
}

fn extent(headers: &HeaderMap, media: &str) -> Result<u64> {
    if single(headers, "content-type").map(HeaderValue::as_bytes) != Some(media.as_bytes())
        || headers.contains_key("content-range")
        || headers.contains_key("transfer-encoding")
        || (headers.contains_key("content-encoding")
            && single(headers, "content-encoding").map(HeaderValue::as_bytes)
                != Some(&b"identity"[..]))
    {
        return Err(invalid("unexpected R3 response framing/media"));
    }
    let length = single(headers, "content-length")
        .ok_or_else(|| invalid("missing exact content length"))?
        .to_str()?;
    if length.is_empty() || !length.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("invalid exact R3 length"));
    }
    Ok(length.parse()?)
}

async fn has_trailers(stream: &mut RecvStream) -> Result<bool> {
    Ok(timeout(IDLE, stream.trailers())
        .await??
        .is_some_and(|headers| !headers.is_empty()))
}

async fn collect_json(response: Response<RecvStream>) -> Result<Vec<u8>> {
    collect_bounded_json(response, r3::MAX_JSON).await
}

async fn collect_bounded_json(response: Response<RecvStream>, limit: u64) -> Result<Vec<u8>> {
    let length = extent(response.headers(), "application/json")?;
    if length > limit {
        return Err(invalid("R3 JSON exceeded its budget"));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(length)?);
    let mut stream = response.into_body();
    while let Some(part) = timeout(IDLE, stream.data()).await? {
        let part = part?;
        if bytes.len() as u64 + part.len() as u64 > length {
            return Err(invalid("R3 JSON exceeded length"));
        }
        bytes.extend_from_slice(&part);
        stream.flow_control().release_capacity(part.len())?;
    }
    if bytes.len() as u64 != length || has_trailers(&mut stream).await? {
        return Err(invalid("short R3 JSON or unsupported trailers"));
    }
    Ok(bytes)
}

async fn send_complete(tx: &mut SendStream<Bytes>, file: File, expected: &Body) -> Result<()> {
    let mut f = tokio::fs::File::from_std(file);
    let mut remaining = expected.length;
    let mut digest = Sha256::new();
    let mut slab = vec![0; CHUNK];
    while remaining != 0 {
        let limit = usize::try_from(remaining.min(CHUNK as u64))?;
        let read = f.read(&mut slab[..limit]).await?;
        if read == 0 {
            return Err(invalid("sealed file ended early"));
        }
        remaining -= read as u64;
        digest.update(&slab[..read]);
        let mut part = Bytes::copy_from_slice(&slab[..read]);
        while !part.is_empty() {
            tx.reserve_capacity(part.len());
            let capacity = if tx.capacity() != 0 {
                tx.capacity()
            } else {
                timeout(IDLE, poll_fn(|cx| tx.poll_capacity(cx)))
                    .await?
                    .ok_or_else(|| invalid("HTTP/2 stream closed before complete body"))??
            };
            if capacity > 0 {
                let slice = part.split_to(capacity.min(part.len()));
                tx.send_data(slice, false)?;
            }
        }
    }
    if f.read(&mut slab[..1]).await? != 0 || hex(&digest.finalize()) != expected.sha256.as_str() {
        return Err(invalid("sealed body changed during exact H2 send"));
    }
    tx.send_data(Bytes::new(), true)?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/origin_cases.rs"]
mod tests;
