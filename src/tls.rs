use crate::model::Host;
use rcgen::generate_simple_self_signed;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::aws_lc_rs::sign::any_supported_type;
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{ClientConfig, DigitallySignedStruct, Error, ServerConfig, ServerConnection, SignatureScheme};
use std::fs;
use std::io::{self, BufReader, Cursor};
use std::net::TcpStream;
use std::sync::OnceLock;
use std::sync::Arc;

static TRANSPORT_SERVER_CONFIG: OnceLock<Arc<ServerConfig>> = OnceLock::new();
static TRANSPORT_CLIENT_CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();

pub fn build_server_config(hosts: &[Host]) -> io::Result<Option<Arc<ServerConfig>>> {
    let resolver = Arc::new(HostCertResolver::from_hosts(hosts)?);
    if resolver.is_empty() {
        return Ok(None);
    }
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Some(Arc::new(config)))
}

pub fn sniff_sni(stream: &TcpStream) -> io::Result<Option<String>> {
    let mut buf = vec![0_u8; 64 * 1024];
    let n = stream.peek(&mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    buf.truncate(n);
    let mut cursor = Cursor::new(buf);
    let mut acceptor = rustls::server::Acceptor::default();
    let _ = acceptor.read_tls(&mut cursor)?;
    match acceptor
        .accept()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, format!("{err:?}")))?
    {
        Some(accepted) => Ok(accepted
            .client_hello()
            .server_name()
            .map(|name| name.to_ascii_lowercase())),
        None => Ok(None),
    }
}

pub fn handshake(stream: TcpStream, config: Arc<ServerConfig>) -> io::Result<rustls::StreamOwned<ServerConnection, TcpStream>> {
    let conn = ServerConnection::new(config).map_err(invalid_data)?;
    let mut tls = rustls::StreamOwned::new(conn, stream);
    while tls.conn.is_handshaking() {
        let _ = tls.conn.complete_io(&mut tls.sock)?;
    }
    Ok(tls)
}

pub fn transport_server_config() -> io::Result<Arc<ServerConfig>> {
    if let Some(config) = TRANSPORT_SERVER_CONFIG.get() {
        return Ok(Arc::clone(config));
    }

    let generated = generate_simple_self_signed(vec!["rustnps-bridge".to_string()])
        .map_err(invalid_data)?;
    let cert_pem = generated.cert.pem();
    let key_pem = generated.key_pair.serialize_pem();

    let mut cert_reader = BufReader::new(Cursor::new(cert_pem.into_bytes()));
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()?;
    let mut key_reader = BufReader::new(Cursor::new(key_pem.into_bytes()));
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_reader)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing transport private key"))?;

    let config = Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(invalid_data)?,
    );
    let _ = TRANSPORT_SERVER_CONFIG.set(Arc::clone(&config));
    Ok(config)
}

pub fn transport_client_config() -> Arc<ClientConfig> {
    TRANSPORT_CLIENT_CONFIG
        .get_or_init(|| {
            Arc::new(
                ClientConfig::builder()
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(NoCertificateVerification::default()))
                    .with_no_client_auth(),
            )
        })
        .clone()
}

pub fn transport_server_name() -> ServerName<'static> {
    ServerName::try_from("rustnps-bridge".to_string()).expect("valid transport server name")
}

#[derive(Debug)]
struct HostCertResolver {
    exact: Vec<(String, Arc<CertifiedKey>)>,
    wildcard: Vec<(String, Arc<CertifiedKey>)>,
    default: Option<Arc<CertifiedKey>>,
}

impl HostCertResolver {
    fn from_hosts(hosts: &[Host]) -> io::Result<Self> {
        let mut exact = Vec::new();
        let mut wildcard = Vec::new();
        let mut default = None;
        for host in hosts {
            let Some(certified_key) = load_certified_key(host)? else {
                continue;
            };
            let domains = split_host_patterns(&host.host);
            if domains.is_empty() {
                continue;
            }
            if default.is_none() {
                default = Some(Arc::clone(&certified_key));
            }
            for domain in domains {
                if let Some(suffix) = domain.strip_prefix("*.") {
                    wildcard.push((suffix.to_ascii_lowercase(), Arc::clone(&certified_key)));
                } else {
                    exact.push((domain.to_ascii_lowercase(), Arc::clone(&certified_key)));
                }
            }
        }
        Ok(Self {
            exact,
            wildcard,
            default,
        })
    }

    fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.wildcard.is_empty() && self.default.is_none()
    }
}

impl ResolvesServerCert for HostCertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let server_name = client_hello.server_name()?.trim().trim_end_matches('.').to_ascii_lowercase();
        for (domain, cert) in &self.exact {
            if *domain == server_name {
                return Some(Arc::clone(cert));
            }
        }
        for (suffix, cert) in &self.wildcard {
            if server_name.ends_with(suffix) {
                return Some(Arc::clone(cert));
            }
        }
        self.default.as_ref().map(Arc::clone)
    }
}

fn load_certified_key(host: &Host) -> io::Result<Option<Arc<CertifiedKey>>> {
    if host.cert_file_path.trim().is_empty() || host.key_file_path.trim().is_empty() {
        return Ok(None);
    }
    let cert_pem = read_pem_source(&host.cert_file_path)?;
    let key_pem = read_pem_source(&host.key_file_path)?;

    let mut cert_reader = BufReader::new(Cursor::new(cert_pem));
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty certificate chain",
        ));
    }

    let mut key_reader = BufReader::new(Cursor::new(key_pem));
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_reader)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing private key"))?;
    let signing_key = any_supported_type(&key).map_err(invalid_data)?;
    Ok(Some(Arc::new(CertifiedKey::new(certs, signing_key))))
}

fn read_pem_source(source: &str) -> io::Result<Vec<u8>> {
    let trimmed = source.trim();
    if trimmed.contains("-----BEGIN") {
        return Ok(trimmed.as_bytes().to_vec());
    }
    fs::read(trimmed)
}

fn split_host_patterns(hosts: &str) -> Vec<String> {
    hosts
        .split(|ch: char| ch == ',' || ch == '\n' || ch == ';' || ch.is_whitespace())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn invalid_data(err: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

#[derive(Debug)]
struct NoCertificateVerification {
    provider: rustls::crypto::CryptoProvider,
}

impl ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

impl Default for NoCertificateVerification {
    fn default() -> Self {
        Self {
            provider: rustls::crypto::aws_lc_rs::default_provider(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{build_server_config, handshake};
    use crate::model::Host;
    use rcgen::generate_simple_self_signed;
    use rustls::pki_types::{CertificateDer, ServerName};
    use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn builds_server_config_from_file_paths() {
        let mut host = Host::default();
        host.host = "example.com".to_string();
        host.scheme = "https".to_string();
        host.cert_file_path = "../nps/conf/server.pem".to_string();
        host.key_file_path = "../nps/conf/server.key".to_string();

        let config = build_server_config(&[host]).expect("build tls config");
        assert!(config.is_some());
    }

    #[test]
    fn skips_hosts_without_key_material() {
        let mut host = Host::default();
        host.host = "example.com".to_string();
        host.scheme = "https".to_string();

        let config = build_server_config(&[host]).expect("build tls config");
        assert!(config.is_none());
    }

    #[test]
    fn sni_handshake_selects_matching_certificate() {
        let cert_one = generate_simple_self_signed(vec!["one.test".to_string()]).unwrap();
        let cert_two = generate_simple_self_signed(vec!["two.test".to_string()]).unwrap();

        let cert_one_pem = cert_one.cert.pem();
        let key_one_pem = cert_one.key_pair.serialize_pem();
        let cert_two_pem = cert_two.cert.pem();
        let key_two_pem = cert_two.key_pair.serialize_pem();
        let cert_one_der: CertificateDer<'static> = cert_one.cert.der().clone();
        let cert_two_der: CertificateDer<'static> = cert_two.cert.der().clone();

        let mut host_one = Host::default();
        host_one.host = "one.test".to_string();
        host_one.scheme = "https".to_string();
        host_one.cert_file_path = cert_one_pem;
        host_one.key_file_path = key_one_pem;

        let mut host_two = Host::default();
        host_two.host = "two.test".to_string();
        host_two.scheme = "https".to_string();
        host_two.cert_file_path = cert_two_pem;
        host_two.key_file_path = key_two_pem;

        let server_config = build_server_config(&[host_one, host_two])
            .expect("build tls config")
            .expect("non-empty tls config");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            for _ in 0..2 {
                let (socket, _) = listener.accept().unwrap();
                let mut tls = handshake(socket, Arc::clone(&server_config)).unwrap();
                tls.write_all(b"ok").unwrap();
                tls.flush().unwrap();
            }
        });

        let first = connect_and_capture_cert(addr, "one.test", &[cert_one_der.clone(), cert_two_der.clone()]);
        assert_eq!(first, cert_one_der.as_ref());

        let second = connect_and_capture_cert(addr, "two.test", &[cert_one_der, cert_two_der.clone()]);
        assert_eq!(second, cert_two_der.as_ref());

        server.join().unwrap();
    }

    fn connect_and_capture_cert(
        addr: std::net::SocketAddr,
        server_name: &str,
        trusted: &[CertificateDer<'static>],
    ) -> Vec<u8> {
        let mut roots = RootCertStore::empty();
        for cert in trusted {
            roots.add(cert.clone()).unwrap();
        }
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let name = ServerName::try_from(server_name.to_string()).unwrap();
        let conn = ClientConnection::new(Arc::new(config), name).unwrap();
        let socket = TcpStream::connect(addr).unwrap();
        let mut tls = StreamOwned::new(conn, socket);
        tls.flush().unwrap();
        let mut buf = [0_u8; 2];
        tls.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ok");
        tls.conn
            .peer_certificates()
            .and_then(|certs| certs.first().cloned())
            .map(|cert| cert.as_ref().to_vec())
            .expect("peer certificate")
    }
}