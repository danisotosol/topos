//! Topos native host — bridges the browser extension to the Google Cast protocol.
//! Communicates via stdin/stdout using the Native Messaging protocol (4-byte LE length prefix + JSON).
//! Runs a local HTTP proxy so the Chromecast fetches streams via this machine's IP (same as the browser),
//! which avoids IP-locked token failures.

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::sync::{Arc, RwLock};

// ── Message types ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(tag = "type")]
enum IncomingMessage {
    #[serde(rename = "SCAN_DEVICES")]
    ScanDevices,
    #[serde(rename = "CAST_STREAM")]
    CastStream {
        url: String,
        device_id: String,
        device_ip: String,
        device_port: u16,
        title: String,
        #[serde(default)]
        cookies: String,
        #[serde(default)]
        referer: String,
    },
    #[serde(rename = "STOP_CAST")]
    StopCast,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum OutgoingMessage {
    #[serde(rename = "DEVICES_FOUND")]
    DevicesFound { devices: Vec<Device> },
    #[serde(rename = "CAST_STARTED")]
    CastStarted { device: String, proxy_url: String },
    #[serde(rename = "CAST_ERROR")]
    CastError { error: String },
}

#[derive(Serialize, Deserialize, Clone)]
struct Device {
    id: String,
    name: String,
    ip: String,
    port: u16,
}

// ── Native Messaging I/O ──────────────────────────────────────────────────────

fn read_message() -> Result<String, io::Error> {
    let mut stdin = io::stdin();
    let size = stdin.read_u32::<LittleEndian>()?;
    let mut buf = vec![0u8; size as usize];
    stdin.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn write_message(msg: &str) -> Result<(), io::Error> {
    let mut stdout = io::stdout();
    stdout.write_u32::<LittleEndian>(msg.len() as u32)?;
    stdout.write_all(msg.as_bytes())?;
    stdout.flush()
}

fn send(msg: &OutgoingMessage) {
    match serde_json::to_string(msg) {
        Ok(json) => {
            if let Err(e) = write_message(&json) {
                eprintln!("[topos] write error: {e}");
            }
        }
        Err(e) => eprintln!("[topos] serialize error: {e}"),
    }
}

// ── Local IP detection ────────────────────────────────────────────────────────

fn get_local_ip() -> String {
    use std::net::UdpSocket;
    // UDP connect to external address picks the right outbound interface without sending packets
    let result = UdpSocket::bind("0.0.0.0:0").and_then(|s| {
        s.connect("8.8.8.8:80")?;
        s.local_addr()
    });
    match result {
        Ok(addr) => addr.ip().to_string(),
        Err(_) => "127.0.0.1".to_string(),
    }
}

// ── HLS manifest rewriting ────────────────────────────────────────────────────

// Rewrites every segment/playlist URI in an m3u8 body to route through the local proxy.
// Resolves relative URIs against base_url before encoding, so the proxy receives absolute URLs.
// Also rewrites URI="..." attributes inside tags like #EXT-X-MAP and #EXT-X-KEY.
fn rewrite_m3u8(body: &str, base_url: &str, proxy_base: &str) -> String {
    let base = url::Url::parse(base_url).ok();

    let resolve = |uri: &str| -> String {
        let resolved = if uri.starts_with("http://") || uri.starts_with("https://") {
            uri.to_string()
        } else if let Some(ref b) = base {
            b.join(uri)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| uri.to_string())
        } else {
            uri.to_string()
        };
        format!("{}/proxy?url={}", proxy_base, urlencoding::encode(&resolved))
    };

    body.lines()
        .map(|line| {
            if line.trim().is_empty() {
                return line.to_string();
            }

            if line.starts_with('#') {
                // Rewrite URI="..." inside tags: #EXT-X-MAP, #EXT-X-KEY, #EXT-X-MEDIA, etc.
                rewrite_tag_uris(line, &resolve)
            } else {
                resolve(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// Rewrites every URI="..." occurrence inside an HLS tag line.
fn rewrite_tag_uris(line: &str, resolve: &impl Fn(&str) -> String) -> String {
    let mut result = String::new();
    let mut rest = line;

    while let Some(pos) = rest.find("URI=\"") {
        result.push_str(&rest[..pos + 5]); // include URI="
        rest = &rest[pos + 5..];

        if let Some(end) = rest.find('"') {
            let uri = &rest[..end];
            result.push_str(&resolve(uri));
            result.push('"');
            rest = &rest[end + 1..];
        } else {
            // Malformed tag — leave remainder unchanged
            result.push_str(rest);
            return result;
        }
    }
    result.push_str(rest);
    result
}

// ── Proxy server ──────────────────────────────────────────────────────────────

#[derive(Clone)]
struct ProxyState {
    client: Arc<reqwest::Client>,
    proxy_base: String,
    // Cookies and Referer extracted from the browser for the current cast session
    cookies: Arc<RwLock<String>>,
    referer: Arc<RwLock<String>>,
}

// Responds to CORS preflight (OPTIONS) from the Chromecast's Chrome browser.
// Chrome's Private Network Access policy requires Access-Control-Allow-Private-Network: true
// before it will allow a public HTTPS origin (gstatic.com) to fetch from a private IP.
async fn options_handler() -> axum::response::Response {
    axum::response::Response::builder()
        .status(204)
        .header("access-control-allow-origin", "*")
        .header("access-control-allow-methods", "GET, OPTIONS")
        .header("access-control-allow-headers", "*")
        .header("access-control-allow-private-network", "true")
        .header("access-control-max-age", "86400")
        .body(axum::body::Body::empty())
        .unwrap()
}

async fn proxy_handler(
    axum::extract::State(state): axum::extract::State<ProxyState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let url = match params.get("url") {
        Some(u) => u.clone(),
        None => return (StatusCode::BAD_REQUEST, "missing url param").into_response(),
    };

    eprintln!("[topos proxy] fetching: {}", &url[..url.len().min(100)]);

    let cookies = state.cookies.read().map(|g| g.clone()).unwrap_or_default();
    let referer = state.referer.read().map(|g| g.clone()).unwrap_or_default();

    let mut req = state.client.get(&url);
    if !cookies.is_empty() {
        req = req.header("Cookie", &cookies);
    }
    if !referer.is_empty() {
        req = req.header("Referer", &referer);
    }

    let upstream = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[topos proxy] fetch error for {}: {e}", &url[..url.len().min(80)]);
            return (StatusCode::BAD_GATEWAY, e.to_string()).into_response();
        }
    };

    let status = upstream.status();
    let content_type = upstream
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let is_m3u8 = content_type.to_lowercase().contains("mpegurl")
        || url.contains(".m3u8");

    if is_m3u8 {
        match upstream.text().await {
            Ok(body) => {
                let rewritten = rewrite_m3u8(&body, &url, &state.proxy_base);
                axum::response::Response::builder()
                    .status(status)
                    .header("content-type", "application/x-mpegURL")
                    .header("access-control-allow-origin", "*")
                    .header("access-control-allow-private-network", "true")
                    .body(axum::body::Body::from(rewritten))
                    .unwrap()
            }
            Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
        }
    } else {
        let stream = upstream.bytes_stream();
        axum::response::Response::builder()
            .status(status)
            .header("content-type", content_type)
            .header("access-control-allow-origin", "*")
            .header("access-control-allow-private-network", "true")
            .body(axum::body::Body::from_stream(stream))
            .unwrap()
    }
}

async fn start_proxy() -> (String, Arc<RwLock<String>>, Arc<RwLock<String>>) {
    use axum::{routing::get, Router};

    let listener = tokio::net::TcpListener::bind("0.0.0.0:7070")
        .await
        .expect("proxy: bind failed — is port 7070 already in use?");
    let port = listener.local_addr().expect("proxy: no local addr").port();
    let local_ip = get_local_ip();
    let proxy_base = format!("http://{}:{}", local_ip, port);

    eprintln!("[topos] proxy listening on {}", proxy_base);

    let client = Arc::new(
        reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:109.0) Gecko/20100101 Firefox/115.0")
            .build()
            .expect("proxy: reqwest client init failed"),
    );

    let cookies: Arc<RwLock<String>> = Arc::new(RwLock::new(String::new()));
    let referer: Arc<RwLock<String>> = Arc::new(RwLock::new(String::new()));

    let state = ProxyState {
        client,
        proxy_base: proxy_base.clone(),
        cookies: cookies.clone(),
        referer: referer.clone(),
    };

    let app = Router::new()
        .route("/proxy", get(proxy_handler).options(options_handler))
        .with_state(state);

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("proxy: serve error");
    });

    (proxy_base, cookies, referer)
}

// ── mDNS discovery ────────────────────────────────────────────────────────────

fn scan_cast_devices() -> Vec<Device> {
    use mdns_sd::{ServiceDaemon, ServiceEvent};

    let mdns = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[topos] mDNS init error: {e}");
            return vec![];
        }
    };

    let receiver = match mdns.browse("_googlecast._tcp.local.") {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[topos] mDNS browse error: {e}");
            return vec![];
        }
    };

    let mut devices: Vec<Device> = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);

    loop {
        if std::time::Instant::now() >= deadline {
            break;
        }
        let timeout = deadline - std::time::Instant::now();
        match receiver.recv_timeout(timeout) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let ip = info
                    .get_addresses()
                    .iter()
                    .find(|a| a.is_ipv4())
                    .or_else(|| info.get_addresses().iter().next())
                    .map(|a| a.to_string())
                    .unwrap_or_default();

                let name = info
                    .get_properties()
                    .get_property_val_str("fn")
                    .unwrap_or(info.get_hostname())
                    .to_string();

                let id = info.get_fullname().to_string();

                if !devices.iter().any(|d| d.id == id) {
                    devices.push(Device {
                        id,
                        name,
                        ip,
                        port: info.get_port(),
                    });
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let _ = mdns.shutdown();
    devices
}

// ── Cast ──────────────────────────────────────────────────────────────────────

struct CastSession {
    device: Device,
    session_id: String,
}

fn cast_stream(device: &Device, url: &str, proxy_base: &str) -> Option<CastSession> {
    use rust_cast::channels::media::{Media, StreamType};
    use rust_cast::channels::receiver::CastDeviceApp;
    use rust_cast::CastDevice;

    let content_type = if url.contains(".m3u8") {
        "application/x-mpegURL"
    } else if url.contains(".mpd") {
        "application/dash+xml"
    } else {
        "video/mp4"
    };

    let stream_type = StreamType::Buffered;

    // Route through local proxy: Chromecast fetches via this machine's IP,
    // which is the same IP that got the auth token from the streaming site.
    let proxy_url = format!("{}/proxy?url={}", proxy_base, urlencoding::encode(url));

    eprintln!("[topos] connecting to {}:{}", device.ip, device.port);
    eprintln!("[topos] cast url (proxied): {}", &proxy_url[..proxy_url.len().min(100)]);

    let result = (|| -> Result<String, Box<dyn std::error::Error>> {
        let cast =
            CastDevice::connect_without_host_verification(device.ip.as_str(), device.port)?;

        cast.connection.connect("receiver-0")?;

        let app = cast
            .receiver
            .launch_app(&CastDeviceApp::DefaultMediaReceiver)?;

        cast.connection.connect(app.transport_id.as_str())?;

        cast.media.load(
            app.transport_id.as_str(),
            app.session_id.as_str(),
            &Media {
                content_id: proxy_url,
                content_type: content_type.to_string(),
                stream_type,
                duration: None,
                metadata: None,
            },
        )?;

        Ok(app.session_id)
    })();

    match result {
        Ok(session_id) => {
            let proxy_url = format!("{}/proxy?url={}", proxy_base, urlencoding::encode(url));
            send(&OutgoingMessage::CastStarted {
                device: device.name.clone(),
                proxy_url,
            });
            Some(CastSession {
                device: device.clone(),
                session_id,
            })
        }
        Err(e) => {
            send(&OutgoingMessage::CastError {
                error: e.to_string(),
            });
            None
        }
    }
}

fn stop_cast(session: &CastSession) {
    use rust_cast::CastDevice;

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let cast = CastDevice::connect_without_host_verification(
            session.device.ip.as_str(),
            session.device.port,
        )?;
        cast.connection.connect("receiver-0")?;
        cast.receiver.stop_app(session.session_id.as_str())?;
        Ok(())
    })();

    if let Err(e) = result {
        eprintln!("[topos] stop error: {e}");
    }
}

// ── Main loop ─────────────────────────────────────────────────────────────────

fn main_loop(proxy_base: String, cookies_store: Arc<RwLock<String>>, referer_store: Arc<RwLock<String>>) {
    let mut active_session: Option<CastSession> = None;

    loop {
        match read_message() {
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                eprintln!("[topos] read error: {e}");
                break;
            }
            Ok(json) => match serde_json::from_str::<IncomingMessage>(&json) {
                Err(e) => eprintln!("[topos] unknown message: {e} — {json}"),
                Ok(IncomingMessage::ScanDevices) => {
                    let devices = scan_cast_devices();
                    send(&OutgoingMessage::DevicesFound { devices });
                }
                Ok(IncomingMessage::CastStream {
                    url,
                    device_id,
                    device_ip,
                    device_port,
                    title,
                    cookies,
                    referer,
                }) => {
                    if !cookies.is_empty() {
                        eprintln!("[topos] got {} cookie bytes from browser", cookies.len());
                        if let Ok(mut guard) = cookies_store.write() {
                            *guard = cookies;
                        }
                    }
                    if !referer.is_empty() {
                        eprintln!("[topos] referer: {}", &referer[..referer.len().min(80)]);
                        if let Ok(mut guard) = referer_store.write() {
                            *guard = referer;
                        }
                    }
                    let device = Device {
                        id: device_id,
                        name: title,
                        ip: device_ip,
                        port: device_port,
                    };
                    active_session = cast_stream(&device, &url, &proxy_base);
                }
                Ok(IncomingMessage::StopCast) => {
                    if let Some(ref session) = active_session {
                        stop_cast(session);
                        active_session = None;
                    }
                }
            },
        }
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let (proxy_base, cookies_store, referer_store) = start_proxy().await;

    // Native messaging uses blocking stdin reads — run in a thread so tokio keeps serving the proxy
    if let Err(e) = tokio::task::spawn_blocking(move || main_loop(proxy_base, cookies_store, referer_store)).await {
        eprintln!("[topos] main loop error: {e}");
    }
}
