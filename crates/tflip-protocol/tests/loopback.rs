//! A client and a server talking over TLS on the loopback interface.

use std::net::TcpListener;
use std::path::Path;
use std::thread;
use std::time::Duration;

use tflip_engine::Error;
use tflip_engine::handle::HandleKind;
use tflip_engine::table::{TableSettings, TableType};
use tflip_protocol::client::{Client, ConnectOptions};
use tflip_protocol::rustls::pki_types::CertificateDer;
use tflip_protocol::server::{accept_tls, greet, serve};
use tflip_protocol::tls::Trust;
use tflip_protocol::tls::server::Identity;
use tflip_protocol::wire::Compression;
use tflip_protocol::{Api, Session};
use tflip_storage::value::Value;

/// A server on a free port that serves `clients` connections, each with a
/// session of its own, and the fingerprint clients trust it by.
fn server(clients: usize) -> (u16, [u8; 32]) {
    server_compressing(clients, Compression::Internet)
}

/// A server as [`server`] makes, with the compression setting `compression`.
fn server_compressing(clients: usize, compression: Compression) -> (u16, [u8; 32]) {
    serving(
        Identity::generate(&["localhost"]).unwrap(),
        clients,
        compression,
    )
}

/// A server as [`server`] makes, presenting `identity`.
fn serving(identity: Identity, clients: usize, compression: Compression) -> (u16, [u8; 32]) {
    let fingerprint = identity.fingerprint().unwrap();
    let config = identity.server_config().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for socket in listener.incoming().take(clients) {
            let config = config.clone();
            thread::spawn(move || {
                let Ok(mut stream) = accept_tls(config, socket.unwrap()) else {
                    return;
                };
                let Ok(greeting) = greet(&mut stream, "test server", compression) else {
                    return;
                };
                let session = Session::with_base(greeting.handle_base);
                let _ = serve(&mut stream, greeting.compressed, &session, |_| {});
            });
        }
    });
    (port, fingerprint)
}

fn connect(port: u16, fingerprint: [u8; 32], base: usize) -> Client {
    let options = ConnectOptions::new(Trust::pinned(vec![fingerprint]), Duration::from_secs(5));
    Client::connect("localhost", port, base, "test client", &options).unwrap()
}

fn fixtures() -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/adt/all_types");
    dir.to_str().unwrap().to_owned()
}

#[test]
fn reading_a_table_remotely() {
    let (port, fingerprint) = server(1);
    let client = connect(port, fingerprint, 0x4000_0000);
    assert_eq!(client.server(), "test server");

    let connection = client
        .connect_path(fixtures(), 1, None, None, None)
        .unwrap();
    assert!(
        connection >= 0x4000_0000,
        "handles come from the base asked for"
    );
    assert_eq!(
        client.handle_kind(connection),
        Ok(Some(HandleKind::Connection))
    );
    let settings = TableSettings {
        table_type: TableType::Adt,
        char_type: 1,
        collation: String::new(),
        options: 0,
        ..TableSettings::default()
    };
    let table = client
        .open_table(connection, "all_types.adt".into(), settings)
        .unwrap();
    let info = client.table_info(table).unwrap();
    assert_eq!(info.structure.fields[0].name, "c_char");
    assert_eq!(
        client.table_info(table).unwrap(),
        info,
        "kept by the client"
    );

    client.goto_record(table, 1).unwrap();
    assert_eq!(client.value(table, 1), Ok(Value::Text("abc       ".into())));
    client.skip(table, 1).unwrap();
    assert_eq!(client.position(table).unwrap().record, Some(2));
    assert_eq!(client.goto_record(table, 99), Ok(()));
    let position = client.position(table).unwrap();
    assert!(position.bof && position.eof, "past the last record");
    assert_eq!(client.goto_record(0x1234_5678, 1), Err(Error::InvalidTable));

    assert_eq!(client.handle_kind(table), Ok(Some(HandleKind::Table)));
    assert_eq!(
        client.handle_kind(table),
        Ok(Some(HandleKind::Table)),
        "kept"
    );
    client.close_table(table).unwrap();
    assert_eq!(
        client.table_info(table),
        Err(Error::InvalidTable),
        "forgotten on close"
    );
    assert_eq!(client.handle_kind(table), Ok(None), "forgotten on close");
    client.disconnect(connection).unwrap();
    assert_eq!(
        client.handle_kind(connection),
        Ok(None),
        "forgotten on disconnect"
    );
}

#[test]
fn sessions_are_separate() {
    let (port, fingerprint) = server(2);
    let a = connect(port, fingerprint, 0x4000_0000);
    let b = connect(port, fingerprint, 0x4010_0000);
    let connection = a.connect_path(fixtures(), 1, None, None, None).unwrap();
    assert_eq!(b.handle_kind(connection), Ok(None));
    a.set_date_format(0, "DD.MM.YYYY".into()).unwrap();
    assert_eq!(a.date_format(0).unwrap(), "DD.MM.CCYY");
    assert_eq!(b.date_format(0).unwrap(), "MM/DD/CCYY");
}

#[test]
fn an_unknown_certificate_is_refused() {
    let (port, _) = server(1);
    let options = ConnectOptions::new(Trust::pinned(vec![[0; 32]]), Duration::from_secs(5));
    let refused = Client::connect("localhost", port, 0x4000_0000, "x", &options);
    let error = refused.err().unwrap();
    assert_eq!(error.error, Error::RemoteNotFound);
    assert!(error.reason.contains("TLS"), "{}", error.reason);
}

#[test]
fn a_request_to_a_stalled_server_times_out() {
    // A server that greets a client but then never answers a request.
    let identity = Identity::generate(&["localhost"]).unwrap();
    let fingerprint = identity.fingerprint().unwrap();
    let config = identity.server_config().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let Ok((socket, _)) = listener.accept() else {
            return;
        };
        let Ok(mut stream) = accept_tls(config, socket) else {
            return;
        };
        let Ok(_) = greet(&mut stream, "stalled server", Compression::Internet) else {
            return;
        };
        thread::sleep(Duration::from_secs(10)); // hold the connection, say nothing
    });
    let options = ConnectOptions {
        request_timeout: Some(Duration::from_millis(300)),
        ..ConnectOptions::new(Trust::pinned(vec![fingerprint]), Duration::from_secs(5))
    };
    let client = Client::connect("localhost", port, 0x4000_0000, "test client", &options).unwrap();
    // The call cannot be answered, so the connection counts as lost.
    assert_eq!(
        client.connect_path(fixtures(), 1, None, None, None),
        Err(Error::ConnectionLost)
    );
    assert!(!client.is_connected());
}

#[test]
fn no_server_is_not_found() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let options = ConnectOptions::new(Trust::pinned(vec![]), Duration::from_secs(5));
    let missing = Client::connect("127.0.0.1", port, 0x4000_0000, "x", &options);
    assert_eq!(missing.err().unwrap().error, Error::RemoteNotFound);
}

#[test]
fn a_broken_connection_stays_broken() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let identity = Identity::generate(&["localhost"]).unwrap();
    let fingerprint = identity.fingerprint().unwrap();
    let config = identity.server_config().unwrap();
    // A server that answers the greeting and one request, then hangs up.
    thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        let mut stream = accept_tls(config, socket).unwrap();
        greet(&mut stream, "short-lived", Compression::Internet).unwrap();
        let session = Session::new();
        let request: tflip_protocol::Request =
            tflip_protocol::wire::read_message(&mut stream).unwrap();
        let reply = tflip_protocol::dispatch(&session, request);
        tflip_protocol::wire::write_message(&mut stream, &reply).unwrap();
    });
    let client = connect(port, fingerprint, 0x4000_0000);
    assert_eq!(client.date_format(0).unwrap(), "MM/DD/CCYY");
    assert_eq!(client.date_format(0), Err(Error::ConnectionLost));
    assert!(!client.is_connected());
    assert_eq!(client.date_format(0), Err(Error::ConnectionLost));
}

#[test]
fn another_protocol_version_is_refused() {
    use tflip_protocol::wire::{self, Hello, Welcome};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        greet(&mut socket, "server", Compression::Internet)
            .unwrap_err()
            .kind()
    });
    let mut socket = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let hello = Hello {
        protocol: 999,
        ..Hello::new(0x4000_0000, "future client", Compression::Internet, false)
    };
    wire::write_message(&mut socket, &hello).unwrap();
    let answer: Result<Welcome, Error> = wire::read_message(&mut socket).unwrap();
    assert_eq!(answer, Err(Error::ProtocolMismatch));
    assert_eq!(server.join().unwrap(), std::io::ErrorKind::InvalidData);
}

/// Client and server agree on compression as the original's do, and a
/// compressed connection works like any other.
#[test]
fn connections_are_compressed_as_agreed() {
    let cases = [
        (Compression::Internet, false, Compression::Internet, false),
        (Compression::Internet, true, Compression::Internet, true),
        (Compression::Always, false, Compression::Internet, true),
        (Compression::Internet, false, Compression::Always, true),
        (Compression::Always, false, Compression::Never, false),
        (Compression::Never, true, Compression::Always, false),
    ];
    for (client_setting, internet, server_setting, compressed) in cases {
        let (port, fingerprint) = server_compressing(1, server_setting);
        let options = ConnectOptions {
            compression: client_setting,
            internet,
            ..ConnectOptions::new(Trust::pinned(vec![fingerprint]), Duration::from_secs(5))
        };
        let client = Client::connect("localhost", port, 0x4000_0000, "c", &options).unwrap();
        let case = format!("{client_setting:?} {internet} {server_setting:?}");
        assert_eq!(client.compressed(), compressed, "{case}");
        assert_eq!(client.server_compression(), server_setting, "{case}");
        let connection = client
            .connect_path(fixtures(), 1, None, None, None)
            .unwrap();
        let settings = TableSettings {
            table_type: TableType::Adt,
            ..TableSettings::default()
        };
        let table = client
            .open_table(connection, "all_types.adt".into(), settings)
            .unwrap();
        for _ in 0..3 {
            assert!(client.record_count(table, false, false).is_ok(), "{case}");
        }
        client.disconnect(connection).unwrap();
    }
}

/// A root CA, and an identity with a certificate for `names` it issued through
/// an intermediate CA, presenting the chain.
fn issued(names: &[&str]) -> (CertificateDer<'static>, Identity) {
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    };
    let ca = |name: &str| {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.distinguished_name.push(DnType::CommonName, name);
        params
    };
    let root_key = KeyPair::generate().unwrap();
    let root_params = ca("tflip test root");
    let root = root_params.self_signed(&root_key).unwrap();
    let root_issuer = Issuer::new(root_params, root_key);
    let middle_key = KeyPair::generate().unwrap();
    let middle_params = ca("tflip test intermediate");
    let middle = middle_params.signed_by(&middle_key, &root_issuer).unwrap();
    let middle_issuer = Issuer::new(middle_params, middle_key);
    let leaf_key = KeyPair::generate().unwrap();
    let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    let mut leaf_params = CertificateParams::new(names).unwrap();
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf = leaf_params.signed_by(&leaf_key, &middle_issuer).unwrap();
    let identity = Identity {
        cert_pem: leaf.pem() + &middle.pem(),
        key_pem: leaf_key.serialize_pem(),
    };
    (root.der().clone(), identity)
}

fn trusting(port: u16, trust: Trust) -> Result<Client, String> {
    let options = ConnectOptions::new(trust, Duration::from_secs(5));
    Client::connect("localhost", port, 0x4000_0000, "c", &options).map_err(|error| error.reason)
}

/// A certificate from a CA is trusted if it chains to a trusted root and
/// names the server, with or without the system's roots besides; one that
/// chains to no trusted root, or names another server, is not; and a
/// fingerprint is trusted whatever the certificate.
#[test]
fn certificates_from_a_ca() {
    let (root, identity) = issued(&["localhost"]);
    for system in [false, true] {
        let (port, _) = serving(identity.clone(), 1, Compression::Internet);
        let trust = Trust {
            system,
            roots: vec![root.clone()],
            ..Trust::default()
        };
        let client = trusting(port, trust).unwrap();
        assert_eq!(client.date_format(0).unwrap(), "MM/DD/CCYY");
    }

    // Not chained to a trusted root: the system's do not know the test CA.
    let (port, fingerprint) = serving(identity.clone(), 2, Compression::Internet);
    let untrusted = trusting(
        port,
        Trust {
            system: true,
            ..Trust::default()
        },
    );
    assert!(untrusted.err().unwrap().contains("TLS"));
    // Its fingerprint is trusted all the same.
    let pinned = Trust {
        system: true,
        ..Trust::pinned(vec![fingerprint])
    };
    assert!(trusting(port, pinned).is_ok());

    // Another server's name.
    let (root, elsewhere) = issued(&["elsewhere.example"]);
    let (port, _) = serving(elsewhere, 1, Compression::Internet);
    let trust = Trust {
        roots: vec![root],
        ..Trust::default()
    };
    assert!(trusting(port, trust).err().unwrap().contains("TLS"));

    // A self-signed tflipd certificate is not trusted by the system's roots.
    let (port, _) = server(1);
    assert!(
        trusting(
            port,
            Trust {
                system: true,
                ..Trust::default()
            }
        )
        .is_err()
    );
}

#[test]
fn roots_from_pem() {
    let (root, identity) = issued(&["localhost"]);
    let chain = tflip_protocol::tls::certificates_from_pem(identity.cert_pem.as_bytes()).unwrap();
    assert_eq!(chain.len(), 2, "the certificate and its intermediate");
    assert_ne!(chain[1], root);
    assert!(
        tflip_protocol::tls::certificates_from_pem(b"")
            .unwrap()
            .is_empty()
    );
}
