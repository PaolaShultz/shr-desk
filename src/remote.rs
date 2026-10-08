//! Explicit mutually authenticated QUIC connection to the same authority session.
//! No listener, automatic pairing, media device or command replay.
use crate::{
    local_audio::{
        AuthorityConnection, TransportTiming, trace_add, trace_timing_enabled, trace_us,
    },
    provider,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub bind: SocketAddr,
    pub server: SocketAddr,
    pub server_name: String,
    pub certificate: PathBuf,
    pub private_key: PathBuf,
    pub ca: PathBuf,
    pub server_certificate_sha256: String,
    pub peer_id: String,
}
fn private_address(address: SocketAddr) -> bool {
    match address.ip() {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local(),
    }
}
fn read_credential(path: &Path, private: bool) -> Result<Vec<u8>, String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !path.is_absolute() || !metadata.is_file() || metadata.len() > 65536 {
        return Err("credential path/type/size".into());
    }
    if private && metadata.mode() & 0o077 != 0 {
        return Err("private key must exclude group/other permissions".into());
    }
    std::fs::read(path).map_err(|e| e.to_string())
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    source_epoch: String,
    capability_generation: String,
    map_generation: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Permission {
    Foh,
    Monitor(u32),
    PaConfiguration,
    FxConfiguration,
    OutputRoutes,
    LocalOperatorMonitor,
    TalkbackDestinations,
    TalkbackFoh,
    Analysis,
    Fx,
}
fn scope_permitted(permissions: &[Permission], scope: &str) -> bool {
    permissions.iter().any(|p| match p {
        Permission::Foh => scope == "foh",
        Permission::Monitor(n) => scope == format!("monitor{n}"),
        Permission::FxConfiguration => scope == "fx_configuration",
        Permission::PaConfiguration => scope == "pa_configuration",
        Permission::OutputRoutes => scope == "output_routes",
        Permission::LocalOperatorMonitor => scope == "local_operator_monitor",
        Permission::TalkbackDestinations => scope == "talkback_destinations",
        Permission::TalkbackFoh => scope == "talkback_foh",
        _ => false,
    })
}
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Response {
    Hello {
        contract: String,
        version: u8,
        session: String,
        writer: String,
        peer_id: String,
        policy_generation: String,
        identity: Identity,
        permissions: Vec<Permission>,
        max_datagram: usize,
    },
    Reply {
        session: String,
        payload: Value,
    },
    Refused {
        session: String,
        reason: String,
    },
}
#[derive(Default)]
struct FrameState {
    failure: Option<String>,
}
impl FrameState {
    fn check(&self) -> Result<(), String> {
        self.failure.as_ref().map_or(Ok(()), |e| Err(e.clone()))
    }
    fn retain<T>(&mut self, result: Result<T, String>) -> Result<T, String> {
        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                let first = self.failure.get_or_insert(e).clone();
                Err(first)
            }
        }
    }
    fn reply(
        &mut self,
        document: provider::StrictDocument,
        session: &str,
        timing: &mut Option<TransportTiming>,
    ) -> Result<provider::StrictDocument, String> {
        self.check()?;
        let started = timing.as_ref().map(|_| Instant::now());
        let admitted = reply_admission(document, session);
        add_stage(timing, 4, started);
        match admitted {
            Ok(ReplyAdmission::Payload(document)) => {
                if let Some(t) = timing {
                    trace_add(&mut t.completed_reply_documents, 1, &mut t.overflow);
                }
                Ok(document)
            }
            Ok(ReplyAdmission::Refused(reason)) => Err(format!("remote refused: {reason}")),
            Err(error) => self.retain(Err(error)),
        }
    }
    async fn read<R: tokio::io::AsyncRead + Unpin>(
        &mut self,
        receive: &mut R,
        deadline: Instant,
        available: bool,
        timing: &mut Option<TransportTiming>,
    ) -> Result<Option<provider::StrictDocument>, String> {
        self.check()?;
        let result = read_document_timed(receive, deadline, available, timing).await;
        self.retain(result)
    }
    async fn write<W: tokio::io::AsyncWrite + Unpin>(
        &mut self,
        send: &mut W,
        value: &Value,
        deadline: Instant,
        timing: &mut Option<TransportTiming>,
    ) -> Result<(), String> {
        self.check()?;
        let result = write_timed(send, value, deadline, timing).await;
        self.retain(result)
    }
}
pub struct Connection {
    framing: FrameState,
    runtime: tokio::runtime::Runtime,
    endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    send: quinn::SendStream,
    receive: quinn::RecvStream,
    session: String,
    capability_generation: String,
    map_generation: String,
    pub writer: String,
    pub source_epoch: u64,
    timing: Option<TransportTiming>,
}
impl Connection {
    pub fn connect(config: &Config, scope: &str) -> Result<Self, String> {
        if !private_address(config.bind)
            || !private_address(config.server)
            || config.server.port() == 0
            || config.server_name.is_empty()
            || config.server_name.len() > 253
            || !provider::id(&config.peer_id)
            || config.server_certificate_sha256.len() != 64
            || !config
                .server_certificate_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("explicit private remote endpoint/identity required".into());
        }
        let certificate = read_credential(&config.certificate, false)?;
        let own_fingerprint = format!("{:x}", Sha256::digest(&certificate));
        let key = PrivateKeyDer::try_from(read_credential(&config.private_key, true)?)
            .map_err(str::to_string)?;
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from(read_credential(&config.ca, false)?))
            .map_err(|e| e.to_string())?;
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_client_auth_cert(vec![CertificateDer::from(certificate)], key)
        .map_err(|e| e.to_string())?;
        tls.alpn_protocols = vec![b"gigpies-remote/1".to_vec()];
        tls.enable_early_data = false;
        tls.resumption = rustls::client::Resumption::disabled();
        let crypto =
            quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(|e| e.to_string())?;
        let mut client = quinn::ClientConfig::new(Arc::new(crypto));
        let mut transport = quinn::TransportConfig::default();
        transport.max_concurrent_bidi_streams(1u8.into());
        transport.max_concurrent_uni_streams(0u8.into());
        transport.stream_receive_window(65540u32.into());
        transport.receive_window(65540u32.into());
        transport.send_window(65540);
        transport.datagram_receive_buffer_size(Some(256 * 1232));
        transport.datagram_send_buffer_size(256 * 1232);
        transport.initial_mtu(1200);
        transport.mtu_discovery_config(None);
        transport.max_idle_timeout(Some(quinn::VarInt::from_u32(2000).into()));
        transport.keep_alive_interval(Some(Duration::from_millis(500)));
        client.transport_config(Arc::new(transport));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let (
            endpoint,
            connection,
            send,
            receive,
            session,
            capability_generation,
            map_generation,
            writer,
            source_epoch,
        ) = runtime.block_on(async {
            let mut endpoint = quinn::Endpoint::client(config.bind).map_err(|e| e.to_string())?;
            endpoint.set_default_client_config(client);
            let connection = tokio::time::timeout(
                Duration::from_secs(2),
                endpoint
                    .connect(config.server, &config.server_name)
                    .map_err(|e| e.to_string())?,
            )
            .await
            .map_err(|_| "remote handshake deadline")?
            .map_err(|e| e.to_string())?;
            let identity = connection
                .peer_identity()
                .ok_or("server certificate unavailable")?
                .downcast::<Vec<CertificateDer<'static>>>()
                .map_err(|_| "server certificate type")?;
            if identity.first().is_none_or(|leaf| {
                format!("{:x}", Sha256::digest(leaf.as_ref())) != config.server_certificate_sha256
            }) {
                connection.close(1u8.into(), b"server pin mismatch");
                return Err("server leaf certificate not paired".to_string());
            }
            let mut session_bytes = [0; 8];
            connection
                .export_keying_material(&mut session_bytes, b"gigpies-remote-session-v1", b"")
                .map_err(|_| "TLS session binding")?;
            let expected_session = u64::from_be_bytes(session_bytes);
            let (mut send, mut receive) =
                tokio::time::timeout(Duration::from_secs(2), connection.open_bi())
                    .await
                    .map_err(|_| "remote stream deadline")?
                    .map_err(|e| e.to_string())?;
            write(
                &mut send,
                &json!({"kind":"open","contract":"GP-REMOTE","version":1}),
                Instant::now() + Duration::from_secs(2),
            )
            .await?;
            let response = read(&mut receive, Instant::now() + Duration::from_secs(2), false)
                .await?
                .ok_or("remote hello deadline")?;
            let Response::Hello {
                contract,
                version,
                session,
                writer,
                peer_id,
                policy_generation,
                identity,
                permissions,
                max_datagram,
            } = response
            else {
                return Err("remote hello required".into());
            };
            if contract != "GP-REMOTE"
                || version != 1
                || expected_session == 0
                || provider::counter(&session)? != expected_session
                || peer_id != config.peer_id
                || writer != format!("remote-{}-{expected_session:016x}", &own_fingerprint[..16])
                || provider::counter(&policy_generation)? == 0
                || provider::counter(&identity.capability_generation)? == 0
                || provider::counter(&identity.map_generation)? == 0
                || max_datagram == 0
                || max_datagram > 1232
            {
                return Err("remote hello identity/binding".into());
            }
            let permitted = scope_permitted(&permissions, scope);
            if !permitted {
                return Err("remote peer lacks requested scope permission".into());
            }
            let source_epoch = provider::counter(&identity.source_epoch)?;
            if source_epoch == 0 {
                return Err("zero remote source epoch".into());
            }
            Ok((
                endpoint,
                connection,
                send,
                receive,
                session,
                identity.capability_generation,
                identity.map_generation,
                writer,
                source_epoch,
            ))
        })?;
        let timing = if trace_timing_enabled() {
            Some(TransportTiming {
                session: provider::counter(&session)?,
                ..Default::default()
            })
        } else {
            None
        };
        Ok(Self {
            framing: FrameState::default(),
            runtime,
            endpoint,
            connection,
            send,
            receive,
            session,
            capability_generation,
            map_generation,
            writer,
            source_epoch,
            timing,
        })
    }
    fn close_failed_framing(&self) {
        if self.framing.failure.is_some() {
            self.connection.close(
                0u8.into(),
                b"Desk framing failed; explicit new attachment required",
            );
        }
    }
    fn receive_document_inner(
        &mut self,
        deadline: Instant,
        available: bool,
    ) -> Result<Option<provider::StrictDocument>, String> {
        let result = self.runtime.block_on(self.framing.read(
            &mut self.receive,
            deadline,
            available,
            &mut self.timing,
        ));
        self.close_failed_framing();
        let Some(document) = result? else {
            return Ok(None);
        };
        let result = self
            .framing
            .reply(document, &self.session, &mut self.timing)
            .map(Some);
        self.close_failed_framing();
        result
    }
    fn receive_inner(
        &mut self,
        deadline: Instant,
        available: bool,
    ) -> Result<Option<Vec<u8>>, String> {
        let result = self.receive_document_inner(deadline, available);
        let Some(document) = result? else {
            return Ok(None);
        };
        let started = self.timing.as_ref().map(|_| Instant::now());
        let result = document.into_bytes().map(Some);
        add_stage(&mut self.timing, 5, started);
        result
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.connection
            .close(0u8.into(), b"Desk disconnected; no replay");
        self.endpoint.close(0u8.into(), b"Desk stopped");
    }
}
impl AuthorityConnection for Connection {
    fn held_identity(&self) -> Option<crate::held_proof::Identity> {
        Some(crate::held_proof::Identity {
            session: self.session.clone(),
            epoch: self.source_epoch.to_string(),
            capability: self.capability_generation.clone(),
            map: self.map_generation.clone(),
        })
    }
    fn receive_document_until(
        &mut self,
        deadline: Instant,
    ) -> Result<Option<provider::StrictDocument>, String> {
        self.receive_document_inner(deadline, false)
    }
    fn timing_snapshot(&self) -> Option<TransportTiming> {
        let mut timing = self.timing?;
        let stats = self.connection.stats();
        timing.flow = [
            stats.frame_rx.max_data,
            stats.frame_rx.max_stream_data,
            stats.frame_rx.data_blocked,
            stats.frame_rx.stream_data_blocked,
            stats.frame_tx.max_data,
            stats.frame_tx.max_stream_data,
            stats.frame_tx.data_blocked,
            stats.frame_tx.stream_data_blocked,
        ];
        timing.rtt_us = trace_us(stats.path.rtt);
        timing.lost_packets = stats.path.lost_packets;
        timing.udp_datagrams = [stats.udp_rx.datagrams, stats.udp_tx.datagrams];
        Some(timing)
    }
    fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
        self.framing.check()?;
        let started = self.timing.as_ref().map(|_| Instant::now());
        let payload = provider::parse(bytes)?;
        if !payload["writer"].is_null() && payload["writer"] != self.writer {
            return Err("authenticated writer mismatch".into());
        }
        let envelope = json!({"kind":"command","session":self.session,"capability_generation":self.capability_generation,"payload":payload});
        add_stage(&mut self.timing, 8, started);
        if let Some(t) = &mut self.timing {
            trace_add(&mut t.send_attempts, 1, &mut t.overflow);
        }
        let result = self.runtime.block_on(self.framing.write(
            &mut self.send,
            &envelope,
            deadline,
            &mut self.timing,
        ));
        self.close_failed_framing();
        add_stage(&mut self.timing, 0, started);
        if result.is_ok()
            && let Some(t) = &mut self.timing
        {
            trace_add(&mut t.completed_sends, 1, &mut t.overflow);
        }
        result
    }
    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        self.receive_inner(deadline, false)
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        self.receive_inner(Instant::now() + Duration::from_millis(200), true)
    }
    fn receive_available_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        self.receive_inner(deadline, true)
    }
}
async fn write(
    send: &mut quinn::SendStream,
    value: &Value,
    deadline: Instant,
) -> Result<(), String> {
    write_timed(send, value, deadline, &mut None).await
}
async fn write_timed<W: tokio::io::AsyncWrite + Unpin>(
    send: &mut W,
    value: &Value,
    deadline: Instant,
    timing: &mut Option<TransportTiming>,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let started = timing.as_ref().map(|_| Instant::now());
    let encoded = serde_json::to_vec(value).map_err(|e| e.to_string());
    add_stage(timing, 6, started);
    let bytes = encoded?;
    if bytes.is_empty() || bytes.len() > provider::MAX_BYTES {
        return Err(format!(
            "remote send frame capacity: length={} limit={}",
            bytes.len(),
            provider::MAX_BYTES
        ));
    }
    let started = timing.as_ref().map(|_| Instant::now());
    let result = tokio::time::timeout_at(deadline.into(), async {
        send.write_all(&(bytes.len() as u32).to_be_bytes())
            .await
            .map_err(|e| e.to_string())?;
        send.write_all(&bytes).await.map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "remote write deadline".to_string());
    add_stage(timing, 7, started);
    result?
}
async fn read<R: tokio::io::AsyncRead + Unpin>(
    receive: &mut R,
    deadline: Instant,
    available: bool,
) -> Result<Option<Response>, String> {
    read_timed(receive, deadline, available, &mut None).await
}
fn add_stage(timing: &mut Option<TransportTiming>, stage: usize, started: Option<Instant>) {
    if let (Some(t), Some(start)) = (timing, started) {
        trace_add(
            &mut t.micros[stage],
            trace_us(start.elapsed()),
            &mut t.overflow,
        );
    }
}
async fn read_timed<R: tokio::io::AsyncRead + Unpin>(
    receive: &mut R,
    deadline: Instant,
    available: bool,
    timing: &mut Option<TransportTiming>,
) -> Result<Option<Response>, String> {
    let Some(document) = read_document_timed(receive, deadline, available, timing).await? else {
        return Ok(None);
    };
    let started = timing.as_ref().map(|_| Instant::now());
    let decoded = decode_response_document(document).map(Some);
    add_stage(timing, 4, started);
    decoded
}
async fn read_document_timed<R: tokio::io::AsyncRead + Unpin>(
    receive: &mut R,
    deadline: Instant,
    available: bool,
    timing: &mut Option<TransportTiming>,
) -> Result<Option<provider::StrictDocument>, String> {
    let Some(first) = read_frame_timed(receive, deadline, available, timing).await? else {
        return Ok(None);
    };
    let mut assembly = crate::pages::Assembly::default();
    let mut frame = first;
    loop {
        let started = timing.as_ref().map(|_| Instant::now());
        let offered = assembly.offer_document(frame, Instant::now());
        add_stage(timing, 3, started);
        if let Some(document) = offered? {
            return Ok(Some(document));
        }
        frame = read_frame_timed(receive, deadline, false, timing)
            .await?
            .ok_or("remote snapshot assembly deadline")?;
    }
}
fn decode_response_document(document: provider::StrictDocument) -> Result<Response, String> {
    serde_json::from_value(document.into_parts().0).map_err(|e| e.to_string())
}
#[cfg(test)]
fn decode_response(bytes: &[u8]) -> Result<Response, String> {
    decode_response_document(provider::StrictDocument::parse(bytes)?)
}
#[cfg(test)]
fn accept_reply_document(
    document: provider::StrictDocument,
    session: &str,
    timing: &mut Option<TransportTiming>,
) -> Result<provider::StrictDocument, String> {
    let started = timing.as_ref().map(|_| Instant::now());
    let result = reply_document(document, session);
    add_stage(timing, 4, started);
    if result.is_ok()
        && let Some(t) = timing
    {
        trace_add(&mut t.completed_reply_documents, 1, &mut t.overflow);
    }
    result
}
/// The exact Reply envelope remains checked before its validated subtree moves.
enum ReplyAdmission {
    Payload(provider::StrictDocument),
    Refused(String),
}
#[cfg(test)]
fn reply_document(
    document: provider::StrictDocument,
    expected_session: &str,
) -> Result<provider::StrictDocument, String> {
    match reply_admission(document, expected_session)? {
        ReplyAdmission::Payload(document) => Ok(document),
        ReplyAdmission::Refused(reason) => Err(format!("remote refused: {reason}")),
    }
}
fn reply_admission(
    document: provider::StrictDocument,
    expected_session: &str,
) -> Result<ReplyAdmission, String> {
    if document.value()["kind"] == "reply" {
        provider::keys(document.value(), &["kind", "session", "payload"])?;
        let session = document.value()["session"]
            .as_str()
            .ok_or("remote session type")?;
        if session != expected_session {
            return Err("remote session/response mismatch".into());
        }
        document.subtree("payload").map(ReplyAdmission::Payload)
    } else {
        match decode_response_document(document)? {
            Response::Refused { session, reason } if session == expected_session => {
                Ok(ReplyAdmission::Refused(reason))
            }
            _ => Err("remote session/response mismatch".into()),
        }
    }
}
async fn read_frame_timed<R: tokio::io::AsyncRead + Unpin>(
    receive: &mut R,
    deadline: Instant,
    available: bool,
    timing: &mut Option<TransportTiming>,
) -> Result<Option<Vec<u8>>, String> {
    use tokio::io::AsyncReadExt;
    let mut length = [0; 4];
    let probe_deadline = if available {
        deadline.min(Instant::now() + Duration::from_millis(1))
    } else {
        deadline
    };
    let started = timing.as_ref().map(|_| Instant::now());
    let first =
        tokio::time::timeout_at(probe_deadline.into(), receive.read_exact(&mut length[..1])).await;
    add_stage(timing, 1, started);
    match first {
        Err(_) => return Ok(None),
        Ok(result) => {
            result.map_err(|e| e.to_string())?;
        }
    }
    // Once any prefix arrives a partial-frame timeout is fatal; no frame replay.
    let started = timing.as_ref().map(|_| Instant::now());
    let result = tokio::time::timeout_at(deadline.into(), async {
        receive
            .read_exact(&mut length[1..])
            .await
            .map_err(|e| e.to_string())?;
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > provider::MAX_BYTES {
            return Err(format!(
                "remote receive frame capacity: length={length} limit={} header=0x{length:08x}",
                provider::MAX_BYTES
            ));
        }
        let mut bytes = vec![0; length];
        receive
            .read_exact(&mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        Ok(Some(bytes))
    })
    .await
    .map_err(|_| "remote partial frame deadline".to_string());
    add_stage(timing, 2, started);
    let result = result?;
    if let (Some(t), Ok(Some(bytes))) = (timing, &result) {
        trace_add(&mut t.frames, 1, &mut t.overflow);
        trace_add(&mut t.bytes, bytes.len() as u64, &mut t.overflow);
    }
    result
}

/// Test-only measurement of the exact post-I/O remote receive stages.
#[cfg(test)]
pub(crate) fn benchmark_snapshot_decode_stages(
    frames: Vec<Vec<u8>>,
) -> (provider::StrictDocument, [u128; 3]) {
    use std::hint::black_box;
    let started = Instant::now();
    let mut assembly = crate::pages::Assembly::default();
    let mut whole = None;
    for frame in frames {
        whole = assembly
            .offer_document(black_box(frame), Instant::now())
            .unwrap();
    }
    let assembly_ns = started.elapsed().as_nanos();
    let document = whole.unwrap();
    let session = document.value()["session"].as_str().unwrap().to_owned();
    let started = Instant::now();
    let document = accept_reply_document(black_box(document), &session, &mut None).unwrap();
    let envelope_ns = started.elapsed().as_nanos();
    // No payload serialization/reparse on the actual document path.
    (document, [assembly_ns, envelope_ns, 0])
}

#[cfg(test)]
mod envelope_tests {
    use super::*;
    fn legacy_payload(bytes: &[u8], session: &str) -> Result<Vec<u8>, String> {
        // Frozen old receive admission, independent of the new proof path.
        let value = provider::parse_document(bytes)?;
        let response: Response = serde_json::from_value(value).map_err(|e| e.to_string())?;
        match response {
            Response::Reply {
                session: got,
                payload,
            } if got == session => serde_json::to_vec(&payload).map_err(|e| e.to_string()),
            Response::Refused {
                session: got,
                reason,
            } if got == session => Err(format!("remote refused: {reason}")),
            _ => Err("remote session/response mismatch".into()),
        }
    }
    fn proof_payload(bytes: &[u8], session: &str) -> Result<provider::StrictDocument, String> {
        reply_document(provider::StrictDocument::parse(bytes)?, session)
    }
    #[test]
    fn strict_document_reply_matches_legacy_wire_admission() {
        let mut cases = vec![
            br#"{ "payload": {"escaped":"\u0041","n":18446744073709551615}, "session":"1", "kind":"reply" }"#.to_vec(),
            br#"{"kind":"reply","session":"1","payload":null}"#.to_vec(),
            br#"{"kind":"reply","session":"1","payload":{"x":1,"x":2}}"#.to_vec(),
            br#"{"kind":"reply","session":"1","payload":{"x":1,"\u0078":2}}"#.to_vec(),
            br#"{"kind":"reply","kind":"reply","session":"1","payload":{}}"#.to_vec(),
            br#"{"kind":"reply","session":"1","payload":{"x":1.0}}"#.to_vec(),
            br#"{"kind":"reply","session":"1","payload":{"x":1e0}}"#.to_vec(),
            br#"{"kind":"reply","session":"1","payload":{"x":18446744073709551616}}"#.to_vec(),
            br#"{"kind":"reply","session":"2","payload":{}}"#.to_vec(),
            br#"{"kind":"reply","session":1,"payload":{}}"#.to_vec(),
            br#"{"kind":"reply","session":"1","payload":{},"extra":0}"#.to_vec(),
            br#"{"kind":"reply","session":"1"}"#.to_vec(),
            br#"{"kind":"refused","session":"1","reason":"no"}"#.to_vec(),
            br#"{"kind":"reply","session":"1","payload":{}} {}"#.to_vec(),
            vec![0xff],
        ];
        cases.push(
            format!(
                "{{\"kind\":\"reply\",\"session\":\"1\",\"payload\":{}{}}}",
                "[".repeat(13),
                "]".repeat(13)
            )
            .into_bytes(),
        );
        for bytes in cases {
            let old = legacy_payload(&bytes, "1");
            let new = proof_payload(&bytes, "1");
            assert_eq!(
                old.is_ok(),
                new.is_ok(),
                "{}",
                String::from_utf8_lossy(&bytes)
            );
            if let (Ok(old), Ok(new)) = (old, new) {
                assert_eq!(new.admitted_bytes(), old.len());
                assert_eq!(new.into_bytes().unwrap(), old);
            }
        }
    }
    #[test]
    fn strict_document_canonical_payload_limits_match_old_normalization() {
        for payload_len in [
            provider::MAX_BYTES - 1,
            provider::MAX_BYTES,
            provider::MAX_BYTES + 1,
        ] {
            let mut payload = json!({"capability_version":1,"padding":""});
            let overhead = serde_json::to_vec(&payload).unwrap().len();
            payload["padding"] = json!("x".repeat(payload_len - overhead));
            let envelope =
                serde_json::to_string(&json!({"kind":"reply","session":"1","payload":payload}))
                    .unwrap();
            // Escaped source spellings/whitespace are larger than canonical bytes.
            let wire = format!("  {}  ", envelope.replacen("xxx", "\\u0078xx", 1));
            let old = legacy_payload(wire.as_bytes(), "1").unwrap();
            let proof = proof_payload(wire.as_bytes(), "1").unwrap();
            assert_eq!(old.len(), payload_len);
            assert_eq!(proof.admitted_bytes(), payload_len);
            let old_error = crate::audio::decode_reply(&old).unwrap_err();
            let new_error = crate::audio::decode_reply_document(proof).unwrap_err();
            assert_eq!(old_error, new_error);
            assert_eq!(
                new_error == "legacy frame capacity",
                payload_len > provider::MAX_BYTES
            );
        }
        // Original envelope admission, independently of canonical payload size.
        let base = br#"{"kind":"reply","session":"1","payload":{}}"#;
        for n in [
            provider::MAX_DOCUMENT_BYTES,
            provider::MAX_DOCUMENT_BYTES + 1,
        ] {
            let mut bytes = base.to_vec();
            bytes.resize(n, b' ');
            assert_eq!(
                legacy_payload(&bytes, "1").is_ok(),
                n == provider::MAX_DOCUMENT_BYTES
            );
            assert_eq!(
                proof_payload(&bytes, "1").is_ok(),
                n == provider::MAX_DOCUMENT_BYTES
            );
        }
    }
    #[test]
    fn strict_document_actual_paged_payload_and_byte_adapter_are_equivalent() {
        let pages: Vec<String> = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/gp14/v1/remote-response-pages.json"
        ))
        .unwrap();
        let whole = include_bytes!("../tests/fixtures/gp14/v1/remote-response-whole.json");
        let session = provider::parse_document(whole).unwrap()["session"]
            .as_str()
            .unwrap()
            .to_owned();
        let old = legacy_payload(whole, &session).unwrap();
        let mut bytes_assembly = crate::pages::Assembly::default();
        let mut proof_assembly = crate::pages::Assembly::default();
        let mut found = None;
        for page in pages {
            let old_doc = bytes_assembly
                .offer(page.as_bytes().to_vec(), Instant::now())
                .unwrap();
            let new_doc = proof_assembly
                .offer_document(page.into_bytes(), Instant::now())
                .unwrap();
            assert_eq!(old_doc.is_some(), new_doc.is_some());
            if let Some(new_doc) = new_doc {
                assert_eq!(old_doc.unwrap(), whole);
                found = Some(reply_document(new_doc, &session).unwrap());
            }
        }
        let proof = found.unwrap();
        assert_eq!(proof.admitted_bytes(), old.len());
        let old_reply = crate::audio::decode_reply(&old).unwrap();
        let new_reply = crate::audio::decode_reply_document(proof).unwrap();
        assert_eq!(
            serde_json::to_value(old_reply).unwrap(),
            serde_json::to_value(new_reply).unwrap()
        );
    }
    #[test]
    fn strict_document_real_profiles_preserve_typed_admission_and_target_shape() {
        for payload in [
            include_bytes!("../tests/fixtures/gp15/v1/grant-16-talkback-reply.json").as_slice(),
            include_bytes!("../tests/fixtures/gp15/v1/grant-32-talkback-reply.json").as_slice(),
            include_bytes!("../tests/fixtures/gp15/v1/grant-48-talkback-reply.json").as_slice(),
        ] {
            let value: Value = serde_json::from_slice(payload).unwrap();
            let envelope =
                serde_json::to_vec(&json!({"kind":"reply","session":"1","payload":value})).unwrap();
            let old = legacy_payload(&envelope, "1").unwrap();
            let proof = proof_payload(&envelope, "1").unwrap();
            assert_eq!(
                serde_json::to_value(crate::audio::decode_reply(&old).unwrap()).unwrap(),
                serde_json::to_value(crate::audio::decode_reply_document(proof).unwrap()).unwrap()
            );
        }
        let mut payload: Value = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/gp15/v1/grant-16-talkback-reply.json"
        ))
        .unwrap();
        payload["outcome"]["body"]["snapshot"] = serde_json::from_slice::<Value>(include_bytes!(
            "../tests/fixtures/gp15/v1/raw-snapshot-16-1.json"
        ))
        .unwrap()["authority"]
            .clone();
        payload["outcome"]["body"]["snapshot"]["parameters"][0]["target"]["monitor"] = Value::Null;
        let envelope =
            serde_json::to_vec(&json!({"kind":"reply","session":"1","payload":payload})).unwrap();
        assert!(crate::audio::decode_reply(&legacy_payload(&envelope, "1").unwrap()).is_err());
        assert!(
            crate::audio::decode_reply_document(proof_payload(&envelope, "1").unwrap()).is_err()
        );
    }
    #[tokio::test(flavor = "current_thread")]
    async fn strict_document_receive_counts_documents_not_pages_or_refusals() {
        use tokio::io::AsyncWriteExt;
        let pages: Vec<String> = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/gp14/v1/remote-response-pages.json"
        ))
        .unwrap();
        let session = provider::parse_document(include_bytes!(
            "../tests/fixtures/gp14/v1/remote-response-whole.json"
        ))
        .unwrap()["session"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut framed = Vec::new();
        for page in &pages {
            framed.extend_from_slice(&(page.len() as u32).to_be_bytes());
            framed.extend_from_slice(page.as_bytes());
        }
        let (mut writer, mut receiver) = tokio::io::duplex(framed.len() + 1);
        writer.write_all(&framed).await.unwrap();
        let mut timing = Some(TransportTiming::default());
        let doc = read_document_timed(
            &mut receiver,
            Instant::now() + Duration::from_secs(2),
            false,
            &mut timing,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(timing.unwrap().frames, pages.len() as u64);
        assert_eq!(timing.unwrap().completed_reply_documents, 0);
        accept_reply_document(doc, &session, &mut timing).unwrap();
        assert_eq!(timing.unwrap().completed_reply_documents, 1);
        for bad in [
            br#"{"kind":"reply","session":"wrong","payload":{}}"#.as_slice(),
            br#"{"kind":"refused","session":"1","reason":"no"}"#.as_slice(),
        ] {
            assert!(
                accept_reply_document(
                    provider::StrictDocument::parse(bad).unwrap(),
                    "1",
                    &mut timing
                )
                .is_err()
            );
            assert_eq!(timing.unwrap().completed_reply_documents, 1);
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn timing_trace_preserves_decode_and_partial_frame_failure() {
        use tokio::io::AsyncWriteExt;
        for enabled in [false, true] {
            let bytes = br#"{"kind":"reply","session":"trace-test","payload":{"n":7}}"#;
            let (mut writer, mut receive) = tokio::io::duplex(1024);
            writer
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .await
                .unwrap();
            writer.write_all(bytes).await.unwrap();
            let mut timing = enabled.then(TransportTiming::default);
            let result = read_timed(
                &mut receive,
                Instant::now() + Duration::from_millis(100),
                false,
                &mut timing,
            )
            .await
            .unwrap();
            let Some(Response::Reply { session, payload }) = result else {
                panic!("reply")
            };
            assert_eq!(session, "trace-test");
            assert_eq!(payload["n"], 7);
            if let Some(t) = timing {
                assert_eq!(t.frames, 1);
                assert_eq!(t.bytes, bytes.len() as u64);
                assert!(!t.overflow);
            }
            let before_failure = timing;
            writer.write_all(&[0]).await.unwrap();
            let error = read_timed(
                &mut receive,
                Instant::now() + Duration::from_millis(5),
                false,
                &mut timing,
            )
            .await
            .unwrap_err();
            assert!(error.contains("partial frame deadline"));
            assert_eq!(timing.is_some(), enabled);
            if let (Some(before), Some(after)) = (before_failure, timing) {
                assert!(after.micros[2] > before.micros[2]);
                assert_eq!(after.frames, before.frames);
                assert_eq!(after.bytes, before.bytes);
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn available_partial_frame_and_page_assembly_keep_original_deadline() {
        use tokio::io::AsyncWriteExt;
        for paged in [false, true] {
            let (mut writer, mut receive) = tokio::io::duplex(65_540);
            if paged {
                let pages: Vec<String> = serde_json::from_slice(include_bytes!(
                    "../tests/fixtures/gp14/v1/remote-response-pages.json"
                ))
                .unwrap();
                let first = pages[0].as_bytes();
                writer
                    .write_all(&(first.len() as u32).to_be_bytes())
                    .await
                    .unwrap();
                writer.write_all(first).await.unwrap();
            } else {
                // Available prefix, withheld remainder: do not grant another200ms.
                writer.write_all(&[0]).await.unwrap();
            }
            let deadline = Instant::now() + Duration::from_millis(20);
            // Independent outer guard proves the production parser does not
            // restart a200ms body/next-page budget after an available first byte.
            let result = tokio::time::timeout(
                Duration::from_millis(120),
                read(&mut receive, deadline, true),
            )
            .await
            .expect("frame/assembly exceeded original deadline");
            assert!(result.is_err());
            drop(writer);
        }
    }

    #[test]
    #[ignore = "finite captured producer decode timing; no network or load"]
    fn captured_producer_decode_timing() {
        let pages: Vec<String> = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/gp14/v1/remote-response-pages.json"
        ))
        .unwrap();
        let corpus: Value = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/gp14/final/structure-replies-48.json"
        ))
        .unwrap();
        let structure = serde_json::to_vec(&corpus["exchanges"][0]["reply"]).unwrap();
        for pass in 0..3 {
            let start = Instant::now();
            let mut assembly = crate::pages::Assembly::default();
            let mut whole = None;
            for page in &pages {
                whole = assembly
                    .offer(page.as_bytes().to_vec(), Instant::now())
                    .unwrap();
            }
            let assembled = start.elapsed();
            let Response::Reply { payload, .. } = decode_response(&whole.unwrap()).unwrap() else {
                panic!("reply")
            };
            let bytes = serde_json::to_vec(&payload).unwrap();
            let envelope = start.elapsed();
            let reply = crate::audio::decode_reply(&bytes).unwrap();
            let snapshot = reply.snapshot.unwrap();
            let mut session = crate::audio::Session::new_version(
                &snapshot.authority.show_id,
                snapshot.authority.epoch.parse().unwrap(),
                "decode-timing",
                "foh",
                2,
            )
            .unwrap();
            session.ingest_snapshot(snapshot, 0).unwrap();
            let raw = start.elapsed();
            crate::structure::decode_reply(&structure)
                .unwrap()
                .snapshot
                .unwrap()
                .validate()
                .unwrap();
            eprintln!(
                "captured48 pass={pass} debug={} raw_bytes={} assembly_ms={} envelope_ms={} raw_decode_validate_ms={} structural_decode_validate_ms={} total_ms={}",
                cfg!(debug_assertions),
                bytes.len(),
                assembled.as_millis(),
                (envelope - assembled).as_millis(),
                (raw - envelope).as_millis(),
                (start.elapsed() - raw).as_millis(),
                start.elapsed().as_millis()
            );
        }
    }
    #[test]
    fn large_actual_producer_reply_decodes_only_after_outer_envelope_reassembly() {
        let pages: Vec<String> = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/gp14/v1/remote-response-pages.json"
        ))
        .unwrap();
        let expected = include_bytes!("../tests/fixtures/gp14/v1/remote-response-whole.json");
        let manifest: Value = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/gp14/v1/remote-response-provenance.json"
        ))
        .unwrap();
        let mut framed = Vec::new();
        let mut assembly = crate::pages::Assembly::default();
        for (index, page) in pages.iter().enumerate() {
            framed.extend_from_slice(&(page.len() as u32).to_be_bytes());
            framed.extend_from_slice(page.as_bytes());
            assert!(decode_response(page.as_bytes()).is_err());
            let result = assembly
                .offer(page.as_bytes().to_vec(), Instant::now())
                .unwrap();
            if index + 1 == pages.len() {
                let bytes = result.unwrap();
                assert_eq!(bytes, expected);
                let Response::Reply { session, payload } = decode_response(&bytes).unwrap() else {
                    panic!("reply")
                };
                assert_eq!(session, manifest["session"].as_str().unwrap());
                let snapshot = crate::audio::decode_reply(&serde_json::to_vec(&payload).unwrap())
                    .unwrap()
                    .snapshot
                    .unwrap();
                assert_eq!(snapshot.authority.inputs.len(), 48);
            } else {
                assert!(result.is_none());
            }
        }
        assert_eq!(
            format!("{:x}", Sha256::digest(&framed)),
            manifest["framed_sha256"].as_str().unwrap()
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(expected)),
            manifest["whole_sha256"].as_str().unwrap()
        );
    }
}

#[cfg(test)]
mod scope_permission_tests {
    use super::*;
    #[test]
    fn explicit_remote_reattachment_never_inherits_other_scope_permission() {
        assert!(!scope_permitted(
            &[Permission::Foh, Permission::Monitor(1)],
            "monitor3"
        ));
        assert!(scope_permitted(&[Permission::Monitor(3)], "monitor3"));
        assert!(!scope_permitted(&[Permission::Monitor(3)], "foh"));
        assert!(!scope_permitted(
            &[Permission::Monitor(3)],
            "pa_configuration"
        ));
    }
}
#[cfg(test)]
mod poisoned_frames {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test(flavor = "current_thread")]
    async fn partial_body_timeout_retains_first_error_and_never_consumes_following_frame() {
        let bytes = br#"{"kind":"reply","session":"x","payload":{"n":7}}"#;
        let (mut writer, mut receiver) = tokio::io::duplex(1024);
        writer
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await
            .unwrap();
        writer.write_all(&bytes[..5]).await.unwrap();
        let mut state = FrameState::default();
        let first = state
            .read(
                &mut receiver,
                Instant::now() + Duration::from_millis(5),
                false,
                &mut None,
            )
            .await
            .err()
            .unwrap();
        assert!(first.contains("partial frame deadline"));
        writer.write_all(&bytes[5..]).await.unwrap();
        writer
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await
            .unwrap();
        writer.write_all(bytes).await.unwrap();
        for _ in 0..2 {
            assert_eq!(
                state
                    .read(
                        &mut receiver,
                        Instant::now() + Duration::from_millis(50),
                        false,
                        &mut None
                    )
                    .await
                    .err()
                    .unwrap(),
                first
            );
        }
        let mut remaining = vec![0; bytes.len() - 5];
        receiver.read_exact(&mut remaining).await.unwrap();
        assert_eq!(remaining, &bytes[5..]);
        let mut prefix = [0; 4];
        receiver.read_exact(&mut prefix).await.unwrap();
        assert_eq!(u32::from_be_bytes(prefix) as usize, bytes.len());
    }
    #[tokio::test(flavor = "current_thread")]
    async fn partial_write_timeout_poison_blocks_later_write_and_read() {
        let (mut sender, mut peer) = tokio::io::duplex(8);
        let mut state = FrameState::default();
        let first = state
            .write(
                &mut sender,
                &json!({"payload":"0123456789"}),
                Instant::now() + Duration::from_millis(5),
                &mut None,
            )
            .await
            .unwrap_err();
        assert!(first.contains("write deadline"));
        let mut sent = [0; 8];
        peer.read_exact(&mut sent).await.unwrap();
        assert_eq!(
            state
                .write(
                    &mut sender,
                    &json!({}),
                    Instant::now() + Duration::from_millis(50),
                    &mut None
                )
                .await
                .unwrap_err(),
            first
        );
        assert_eq!(
            state
                .read(
                    &mut sender,
                    Instant::now() + Duration::from_millis(50),
                    false,
                    &mut None
                )
                .await
                .err()
                .unwrap(),
            first
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(5), peer.read_u8())
                .await
                .is_err()
        );
    }
}
#[cfg(test)]
mod application_refusal_recovery {
    use super::*;
    use tokio::io::AsyncWriteExt;
    #[tokio::test(flavor = "current_thread")]
    async fn typed_application_refusal_does_not_poison_following_valid_reply() {
        let (mut writer, mut receiver) = tokio::io::duplex(1024);
        for v in [
            json!({"kind":"refused","session":"1","reason":"optional GP21 unsupported"}),
            json!({"kind":"reply","session":"1","payload":{"n":7}}),
        ] {
            let b = serde_json::to_vec(&v).unwrap();
            writer
                .write_all(&(b.len() as u32).to_be_bytes())
                .await
                .unwrap();
            writer.write_all(&b).await.unwrap();
        }
        let mut state = FrameState::default();
        let refusal = state
            .read(
                &mut receiver,
                Instant::now() + Duration::from_millis(50),
                false,
                &mut None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            state.reply(refusal, "1", &mut None).err().unwrap(),
            "remote refused: optional GP21 unsupported"
        );
        assert!(state.failure.is_none());
        let reply = state
            .read(
                &mut receiver,
                Instant::now() + Duration::from_millis(50),
                false,
                &mut None,
            )
            .await
            .unwrap()
            .unwrap();
        let payload = state.reply(reply, "1", &mut None).unwrap();
        assert_eq!(payload.value()["n"], 7);
        assert!(state.failure.is_none());
    }
}
