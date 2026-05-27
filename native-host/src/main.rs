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
        #[serde(default)]
        subtitle_url: String,
    },
    #[serde(rename = "STOP_CAST")]
    StopCast,
    #[serde(rename = "PAUSE_CAST")]
    PauseCast,
    #[serde(rename = "PLAY_CAST")]
    PlayCast,
    #[serde(rename = "SEEK_CAST")]
    SeekCast { position: f64 },
    #[serde(rename = "QUERY_CAST_STATE")]
    QueryCastState,
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
    #[serde(rename = "CAST_STATE")]
    CastState { player_state: String, current_time: f64, duration: Option<f64> },
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

// Forces DEFAULT=YES and AUTOSELECT=YES on an #EXT-X-MEDIA:TYPE=SUBTITLES line so that
// Shaka Player (DefaultMediaReceiver) activates the track without user interaction.
fn force_subtitle_autoselect(line: &str) -> String {
    let set = |s: &str, attr: &str, val: &str| -> String {
        let upper = attr.to_uppercase();
        if let Some(pos) = s.to_uppercase().find(&format!("{}=", upper)) {
            // Replace existing value
            let after = &s[pos + attr.len() + 1..];
            let end = after.find(',').unwrap_or(after.len());
            format!("{}{}", &s[..pos + attr.len() + 1], format!("{}{}", val, &after[end..]))
        } else {
            // Inject before the first comma or at end
            match s.find(',') {
                Some(p) => format!("{},{}={}{}", &s[..p], attr, val, &s[p..]),
                None => format!("{},{}={}", s, attr, val),
            }
        }
    };
    let line = set(line, "DEFAULT", "YES");
    set(&line, "AUTOSELECT", "YES")
}

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
                let rewritten = rewrite_tag_uris(line, &resolve);
                // Force subtitle tracks to auto-select so DefaultMediaReceiver enables them
                if rewritten.contains("TYPE=SUBTITLES") {
                    force_subtitle_autoselect(&rewritten)
                } else {
                    rewritten
                }
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
async fn options_handler(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let url = params.get("url").map(|u| &u[..u.len().min(80)]).unwrap_or("(no url)");
    eprintln!("[topos proxy] OPTIONS preflight for: {}", url);
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
    // Derive Origin from Referer — some CDNs require it for CORS
    let origin = url::Url::parse(&referer)
        .ok()
        .map(|u| format!("{}://{}", u.scheme(), u.host_str().unwrap_or("")))
        .unwrap_or_default();

    let mut req = state.client.get(&url);
    if !cookies.is_empty() {
        req = req.header("Cookie", &cookies);
    }
    if !referer.is_empty() {
        req = req.header("Referer", &referer);
    }
    if !origin.is_empty() {
        req = req.header("Origin", &origin);
    }

    let upstream = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[topos proxy] fetch error for {}: {e}", &url[..url.len().min(80)]);
            return (StatusCode::BAD_GATEWAY, e.to_string()).into_response();
        }
    };

    let status = upstream.status();
    // Log every upstream response so we can spot 403/404 for segments
    eprintln!("[topos proxy] {} → {}", &url[..url.len().min(80)], status);

    // Use the final URL after any redirects as the rewriting base.
    // If reqwest followed a redirect, relative URIs in the m3u8 must be
    // resolved against the redirect target, not the original URL.
    let final_url = upstream.url().to_string();

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
                let rewritten = rewrite_m3u8(&body, &final_url, &state.proxy_base);
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

// ── Raw Cast protocol (for LOAD with subtitle tracks) ─────────────────────
//
// rust-cast 0.14 does not support the `tracks`/`activeTrackIds` fields in
// the Cast LOAD command. External VTT subtitles cannot be added via
// EDIT_TRACKS_INFO after the fact — they must be declared at load time.
// We implement a minimal raw sender: manual protobuf + openssl TLS, reusing
// the device connection already established by rust-cast for connect/launch.

fn varint_encode(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut b = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 { b |= 0x80; }
        out.push(b);
        if v == 0 { break; }
    }
    out
}

fn pb_string(field: u32, s: &str) -> Vec<u8> {
    let tag = (field << 3) | 2; // wire type 2 = length-delimited
    let mut out = varint_encode(tag as u64);
    out.extend(varint_encode(s.len() as u64));
    out.extend_from_slice(s.as_bytes());
    out
}

fn pb_int32(field: u32, v: i32) -> Vec<u8> {
    let tag = field << 3; // wire type 0 = varint
    let mut out = varint_encode(tag as u64);
    out.extend(varint_encode(v as u64));
    out
}

// Serialises a CastMessage protobuf manually (avoids pulling in prost/protobuf).
// CastMessage fields: 1=protocol_version, 2=source_id, 3=destination_id,
//                     4=namespace, 5=payload_type, 6=payload_utf8
fn build_cast_message(source: &str, dest: &str, ns: &str, payload: &str) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.extend(pb_int32(1, 0));         // CASTV2_1_0
    msg.extend(pb_string(2, source));
    msg.extend(pb_string(3, dest));
    msg.extend(pb_string(4, ns));
    msg.extend(pb_int32(5, 0));         // STRING payload
    msg.extend(pb_string(6, payload));
    msg
}

fn write_cast_frame(stream: &mut impl std::io::Write, bytes: &[u8]) -> Result<(), std::io::Error> {
    use byteorder::{BigEndian, WriteBytesExt};
    stream.write_u32::<BigEndian>(bytes.len() as u32)?;
    stream.write_all(bytes)?;
    Ok(())
}

fn cast_connect_msg(source: &str, dest: &str) -> Vec<u8> {
    build_cast_message(
        source, dest,
        "urn:x-cast:com.google.cast.tp.connection",
        r#"{"type":"CONNECT","userAgent":"Topos/1.0"}"#,
    )
}

// Detect subtitle language code from URL filename (e.g. "spa-42.vtt" → "es")
fn subtitle_language(url: &str) -> &str {
    let filename = url.rsplit('/').next().unwrap_or("");
    let stem = filename.split('.').next().unwrap_or("");
    let code = stem.split('-').next().unwrap_or("und");
    match code {
        "spa" => "es",
        "eng" => "en",
        "por" | "pt-BR" | "pt-PT" => "pt",
        "fra" => "fr",
        "deu" | "ger" => "de",
        "ita" => "it",
        "jpn" => "ja",
        "kor" => "ko",
        "zho" | "chi" | "zh" => "zh",
        other => other,
    }
}

// Opens a fresh TLS connection to the Chromecast and sends a LOAD command that
// includes the subtitle VTT as an external text track with activeTrackIds=[1].
// Called after rust-cast has launched DefaultMediaReceiver and its connection drops.
fn raw_cast_load(
    ip: &str,
    port: u16,
    transport_id: &str,
    session_id: &str,
    content_id: &str,
    content_type: &str,
    subtitle_proxy_url: &str,
    subtitle_lang: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
    use std::net::TcpStream;

    let mut builder = SslConnector::builder(SslMethod::tls())?;
    builder.set_verify(SslVerifyMode::NONE);
    let connector = builder.build();
    let tcp = TcpStream::connect((ip, port))?;
    let mut stream = connector.connect(ip, tcp)?;

    // Virtual channel CONNECT to receiver-0
    let conn_r0 = cast_connect_msg("sender-0", "receiver-0");
    write_cast_frame(&mut stream, &conn_r0)?;

    // Virtual channel CONNECT to the app transport
    let conn_tp = cast_connect_msg("sender-0", transport_id);
    write_cast_frame(&mut stream, &conn_tp)?;

    // LOAD with tracks + activeTrackIds
    let load_json = serde_json::json!({
        "type": "LOAD",
        "requestId": 1,
        "sessionId": session_id,
        "autoplay": true,
        "media": {
            "contentId": content_id,
            "contentType": content_type,
            "streamType": "BUFFERED",
            "tracks": [{
                "trackId": 1,
                "type": "TEXT",
                "trackContentId": subtitle_proxy_url,
                "trackContentType": "text/vtt",
                "subtype": "SUBTITLES",
                "language": subtitle_lang,
                "name": subtitle_lang
            }]
        },
        "activeTrackIds": [1],
        "textTrackStyle": {
            "backgroundColor": "#00000000",
            "edgeColor": "#000000FF",
            "edgeType": "OUTLINE",
            "fontFamily": "SANS_SERIF",
            "fontScale": 1.0,
            "foregroundColor": "#FFFFFFFF",
            "windowType": "NONE"
        }
    })
    .to_string();

    let load_msg = build_cast_message(
        "sender-0",
        transport_id,
        "urn:x-cast:com.google.cast.media",
        &load_json,
    );
    write_cast_frame(&mut stream, &load_msg)?;
    stream.flush()?;

    eprintln!("[topos] raw LOAD with subtitle track sent (lang={})", subtitle_lang);
    Ok(())
}

// ── Cast ──────────────────────────────────────────────────────────────────────

struct CastSession {
    device: Device,
    session_id: String,
}

fn cast_stream(device: &Device, url: &str, proxy_base: &str, subtitle_url: &str) -> Option<CastSession> {
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

    // Route through local proxy: Chromecast fetches via this machine's IP,
    // which is the same IP that got the auth token from the streaming site.
    let proxy_url = format!("{}/proxy?url={}", proxy_base, urlencoding::encode(url));
    let subtitle_proxy_url = if subtitle_url.is_empty() {
        String::new()
    } else {
        format!("{}/proxy?url={}", proxy_base, urlencoding::encode(subtitle_url))
    };

    eprintln!("[topos] connecting to {}:{}", device.ip, device.port);
    eprintln!("[topos] cast url (proxied): {}", &proxy_url[..proxy_url.len().min(100)]);

    let has_subtitle = !subtitle_url.is_empty();

    // When subtitles are present we send a raw LOAD (rust-cast 0.14 lacks tracks support).
    // Critical: raw_cast_load must be called INSIDE the closure, while the rust-cast
    // CastDevice is still alive — if it drops first, the Chromecast may kill the app
    // and the LOAD goes to a dead transport_id.
    let result = (|| -> Result<(String, String), Box<dyn std::error::Error>> {
        let cast =
            CastDevice::connect_without_host_verification(device.ip.as_str(), device.port)?;

        cast.connection.connect("receiver-0")?;

        let app = cast
            .receiver
            .launch_app(&CastDeviceApp::DefaultMediaReceiver)?;

        cast.connection.connect(app.transport_id.as_str())?;

        if has_subtitle {
            let lang = subtitle_language(subtitle_url);
            raw_cast_load(
                &device.ip,
                device.port,
                app.transport_id.as_str(),
                app.session_id.as_str(),
                &proxy_url,
                content_type,
                &subtitle_proxy_url,
                lang,
            )?;
        } else {
            cast.media.load(
                app.transport_id.as_str(),
                app.session_id.as_str(),
                &Media {
                    content_id: proxy_url.clone(),
                    content_type: content_type.to_string(),
                    stream_type: StreamType::Buffered,
                    duration: None,
                    metadata: None,
                },
            )?;
        }

        Ok((app.session_id, app.transport_id))
    })();

    match result {
        Ok((session_id, _transport_id)) => {
            send(&OutgoingMessage::CastStarted {
                device: device.name.clone(),
                proxy_url: proxy_url.clone(),
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

// ── Playback control ──────────────────────────────────────────────────────

// Reconnects to the Chromecast, finds the active media session, runs `action`,
// then queries the updated player state to return to the extension.
// Uses launch_app (idempotent — returns existing session without restarting it).
fn reconnect_media<F>(session: &CastSession, action: F)
where
    F: FnOnce(&rust_cast::CastDevice, &str, i32) -> Result<(), Box<dyn std::error::Error>>,
{
    use rust_cast::channels::media::PlayerState;
    use rust_cast::channels::receiver::CastDeviceApp;
    use rust_cast::CastDevice;

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let cast =
            CastDevice::connect_without_host_verification(session.device.ip.as_str(), session.device.port)?;
        cast.connection.connect("receiver-0")?;

        let app = cast.receiver.launch_app(&CastDeviceApp::DefaultMediaReceiver)?;
        cast.connection.connect(app.transport_id.as_str())?;

        let response = cast.media.get_status(app.transport_id.as_str(), None)?;
        let status = response.entries.first().ok_or("no active media session")?;
        let media_session_id = status.media_session_id;

        action(&cast, app.transport_id.as_str(), media_session_id)?;

        // Query updated state and report it to the extension
        let updated = cast.media.get_status(app.transport_id.as_str(), None)?;
        let s = updated.entries.first();
        let player_state = s
            .map(|s| match s.player_state {
                PlayerState::Playing => "PLAYING",
                PlayerState::Paused => "PAUSED",
                PlayerState::Buffering => "BUFFERING",
                PlayerState::Idle => "IDLE",
            })
            .unwrap_or("UNKNOWN");
        let current_time = s.and_then(|s| s.current_time).unwrap_or(0.0) as f64;
        let duration = s
            .and_then(|s| s.media.as_ref())
            .and_then(|m| m.duration)
            .map(|d| d as f64);

        send(&OutgoingMessage::CastState {
            player_state: player_state.to_string(),
            current_time,
            duration,
        });
        Ok(())
    })();

    if let Err(e) = result {
        eprintln!("[topos] media control error: {e}");
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
                    subtitle_url,
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
                    if !subtitle_url.is_empty() {
                        eprintln!("[topos] subtitle URL from browser: {}", &subtitle_url[..subtitle_url.len().min(100)]);
                    } else {
                        eprintln!("[topos] no subtitle URL detected (subtitles may be embedded in HLS manifest)");
                    }
                    let device = Device {
                        id: device_id,
                        name: title,
                        ip: device_ip,
                        port: device_port,
                    };
                    active_session = cast_stream(&device, &url, &proxy_base, &subtitle_url);
                }
                Ok(IncomingMessage::StopCast) => {
                    if let Some(ref session) = active_session {
                        stop_cast(session);
                        active_session = None;
                    }
                }
                Ok(IncomingMessage::PauseCast) => {
                    if let Some(ref session) = active_session {
                        reconnect_media(session, |cast, dest, media_id| {
                            cast.media.pause(dest, media_id)?;
                            Ok(())
                        });
                    }
                }
                Ok(IncomingMessage::PlayCast) => {
                    if let Some(ref session) = active_session {
                        reconnect_media(session, |cast, dest, media_id| {
                            cast.media.play(dest, media_id)?;
                            Ok(())
                        });
                    }
                }
                Ok(IncomingMessage::SeekCast { position }) => {
                    if let Some(ref session) = active_session {
                        reconnect_media(session, |cast, dest, media_id| {
                            use rust_cast::channels::media::ResumeState;
                            cast.media.seek(dest, media_id, Some(position as f32), Some(ResumeState::PlaybackStart))?;
                            Ok(())
                        });
                    }
                }
                Ok(IncomingMessage::QueryCastState) => {
                    if let Some(ref session) = active_session {
                        reconnect_media(session, |_cast, _dest, _media_id| Ok(()));
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
