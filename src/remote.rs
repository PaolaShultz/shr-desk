//! Explicit mutually authenticated QUIC connection to the same authority session.
//! No listener, automatic pairing, media device or command replay.
use crate::{local_audio::AuthorityConnection, provider};
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
    OutputRoutes,
    LocalOperatorMonitor,
    TalkbackDestinations,
    TalkbackFoh,
    Analysis,
    Fx,
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
pub struct Connection {
    runtime: tokio::runtime::Runtime,
    endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    send: quinn::SendStream,
    receive: quinn::RecvStream,
    session: String,
    capability_generation: String,
    pub writer: String,
    pub source_epoch: u64,
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
            let permitted = permissions.iter().any(|p| match p {
                Permission::Foh => scope == "foh",
                Permission::Monitor(n) => scope == format!("monitor{n}"),
                Permission::PaConfiguration => scope == "pa_configuration",
                Permission::OutputRoutes => scope == "output_routes",
                Permission::LocalOperatorMonitor => scope == "local_operator_monitor",
                Permission::TalkbackDestinations => scope == "talkback_destinations",
                Permission::TalkbackFoh => scope == "talkback_foh",
                _ => false,
            });
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
                writer,
                source_epoch,
            ))
        })?;
        Ok(Self {
            runtime,
            endpoint,
            connection,
            send,
            receive,
            session,
            capability_generation,
            writer,
            source_epoch,
        })
    }
    fn receive_inner(
        &mut self,
        deadline: Instant,
        available: bool,
    ) -> Result<Option<Vec<u8>>, String> {
        match self
            .runtime
            .block_on(read(&mut self.receive, deadline, available))?
        {
            None => Ok(None),
            Some(Response::Reply { session, payload }) if session == self.session => {
                serde_json::to_vec(&payload)
                    .map(Some)
                    .map_err(|e| e.to_string())
            }
            Some(Response::Refused { session, reason }) if session == self.session => {
                Err(format!("remote refused: {reason}"))
            }
            _ => Err("remote session/response mismatch".into()),
        }
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
    fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
        let payload = provider::parse(bytes)?;
        if !payload["writer"].is_null() && payload["writer"] != self.writer {
            return Err("authenticated writer mismatch".into());
        }
        self.runtime.block_on(write(&mut self.send, &json!({"kind":"command","session":self.session,"capability_generation":self.capability_generation,"payload":payload}), deadline))
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
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.is_empty() || bytes.len() > provider::MAX_BYTES {
        return Err("remote frame capacity".into());
    }
    tokio::time::timeout_at(deadline.into(), async {
        send.write_all(&(bytes.len() as u32).to_be_bytes())
            .await
            .map_err(|e| e.to_string())?;
        send.write_all(&bytes).await.map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "remote write deadline".to_string())?
}
async fn read<R: tokio::io::AsyncRead + Unpin>(
    receive: &mut R,
    deadline: Instant,
    available: bool,
) -> Result<Option<Response>, String> {
    let Some(first) = read_frame(receive, deadline, available).await? else {
        return Ok(None);
    };
    // The producer segments the complete remote Response, outside its payload.
    // Reassemble that immutable envelope before decoding the session binding.
    let mut assembly = crate::pages::Assembly::default();
    let mut frame = first;
    loop {
        if let Some(bytes) = assembly.offer(frame, Instant::now())? {
            return decode_response(&bytes).map(Some);
        }
        frame = read_frame(receive, deadline, false)
            .await?
            .ok_or("remote snapshot assembly deadline")?;
    }
}
fn decode_response(bytes: &[u8]) -> Result<Response, String> {
    let value = provider::parse_document(bytes)?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}
async fn read_frame<R: tokio::io::AsyncRead + Unpin>(
    receive: &mut R,
    deadline: Instant,
    available: bool,
) -> Result<Option<Vec<u8>>, String> {
    use tokio::io::AsyncReadExt;
    let mut length = [0; 4];
    let probe_deadline = if available {
        deadline.min(Instant::now() + Duration::from_millis(1))
    } else {
        deadline
    };
    match tokio::time::timeout_at(probe_deadline.into(), receive.read_exact(&mut length[..1])).await
    {
        Err(_) => return Ok(None),
        Ok(result) => {
            result.map_err(|e| e.to_string())?;
        }
    }
    // Once any prefix arrives a partial-frame timeout is fatal; no frame replay.
    tokio::time::timeout_at(deadline.into(), async {
        receive
            .read_exact(&mut length[1..])
            .await
            .map_err(|e| e.to_string())?;
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > provider::MAX_BYTES {
            return Err("remote frame capacity".into());
        }
        let mut bytes = vec![0; length];
        receive
            .read_exact(&mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        Ok(Some(bytes))
    })
    .await
    .map_err(|_| "remote partial frame deadline".to_string())?
}

/// Test-only measurement of the exact post-I/O remote receive stages.
#[cfg(test)]
pub(crate) fn benchmark_snapshot_decode_stages(frames: Vec<Vec<u8>>) -> (Vec<u8>, [u128; 3]) {
    use std::hint::black_box;
    let started = Instant::now();
    let mut assembly = crate::pages::Assembly::default();
    let mut whole = None;
    for frame in frames {
        whole = assembly.offer(black_box(frame), Instant::now()).unwrap();
    }
    let assembly_ns = started.elapsed().as_nanos();
    let whole = whole.unwrap();
    let started = Instant::now();
    let Response::Reply { payload, .. } = decode_response(black_box(&whole)).unwrap() else {
        panic!("benchmark reply envelope")
    };
    let envelope_ns = started.elapsed().as_nanos();
    let started = Instant::now();
    let bytes = serde_json::to_vec(black_box(&payload)).unwrap();
    let serialization_ns = started.elapsed().as_nanos();
    (bytes, [assembly_ns, envelope_ns, serialization_ns])
}

#[cfg(test)]
mod envelope_tests {
    use super::*;
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
