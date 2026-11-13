//! `AdsFindServers`: discovering Advantage Database Servers by IPv4 multicast.
//!
//! The client multicasts a 50-byte request to `224.0.1.55:2989`; each server
//! replies (by unicast) with its advertised addresses, port and name. tflip
//! gathers those into the 13-column in-memory table `AdsFindServers` returns.
//!
//! The reply carries only the addresses, port and (v10+) server name; the ten
//! "detail" columns the original engine fills — serial number, version, owner,
//! user count, and so on — it gathers afterwards by opening a full client
//! connection to each discovered server. tflip has no remote-server client, so
//! it leaves those blank, exactly as the original's own `ADS_FS_MULTICAST_ONLY`
//! option does. Discovery is IPv4 only; ADS has no IPv6 discovery group.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use tflip_engine::Error;
use tflip_engine::handle::Handle;
use tflip_storage::build::Column;
use tflip_storage::field::FieldType;
use tflip_storage::value::Value;

use crate::LOCAL;

/// The discovery multicast group and port (documented by `AdsFindServers`).
const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 1, 55);
const PORT: u16 = 2989;
/// How many times the request is sent (the original sends four), and how long
/// replies are gathered after each.
const RETRIES: u32 = 4;
const WAIT: Duration = Duration::from_millis(250);

/// One advertised server address from a discovery reply.
struct Found {
    ip: Ipv4Addr,
    port: u16,
    /// The address the reply came from (the result's "Multicast Addr").
    responder: Ipv4Addr,
    name: String,
}

/// Sends the discovery request a few times and gathers the replies. Any
/// network failure yields no servers rather than an error.
fn discover() -> Vec<Found> {
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
        return Vec::new();
    };
    let _ = socket.set_multicast_ttl_v4(1);
    let mut request = [0u8; 50];
    request[0] = 0x01;
    request[1] = 0x2a;
    let destination = SocketAddrV4::new(GROUP, PORT);
    let mut found: Vec<Found> = Vec::new();
    for _ in 0..RETRIES {
        if socket.send_to(&request, destination).is_err() {
            break;
        }
        let deadline = Instant::now() + WAIT;
        let mut buffer = [0u8; 2048];
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || socket.set_read_timeout(Some(remaining)).is_err() {
                break;
            }
            match socket.recv_from(&mut buffer) {
                Ok((length, SocketAddr::V4(source))) => {
                    parse_reply(&buffer[..length], *source.ip(), &mut found);
                }
                Ok((_, SocketAddr::V6(_))) => {}
                Err(_) => break,
            }
        }
    }
    found.sort_by_key(|f| (f.ip.octets(), f.port));
    found.dedup_by_key(|f| (f.ip.octets(), f.port));
    found
}

/// Parses one reply: a `01` marker, a one-byte address count, then one
/// `sockaddr_in` per address (2-byte `AF_INET` family, big-endian port,
/// 4-byte address, 8 bytes of padding), then a 4-byte server id, a 4-byte
/// flag and the NUL-terminated server name.
fn parse_reply(data: &[u8], responder: Ipv4Addr, found: &mut Vec<Found>) {
    if data.len() < 2 || data[0] != 0x01 {
        return;
    }
    let count = usize::from(data[1]);
    let mut offset = 2usize;
    let mut addresses = Vec::new();
    for _ in 0..count {
        let Some(record) = data.get(offset..offset + 16) else {
            return;
        };
        let port = u16::from_be_bytes([record[2], record[3]]);
        let ip = Ipv4Addr::new(record[4], record[5], record[6], record[7]);
        addresses.push((ip, port));
        offset += 16;
    }
    // A 4-byte server id and a 4-byte flag precede the name.
    offset += 8;
    let name = data
        .get(offset..)
        .map(|rest| {
            let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            String::from_utf8_lossy(&rest[..end]).into_owned()
        })
        .unwrap_or_default();
    for (ip, port) in addresses {
        found.push(Found {
            ip,
            port,
            responder,
            name: name.clone(),
        });
    }
}

/// The 13 columns of the result, as the original engine's result table has
/// them (names, types and lengths matched against a live server).
fn columns() -> Vec<Column> {
    let text = |name: &str, length: u16| Column {
        name: name.to_owned(),
        field_type: FieldType::Character,
        length,
        decimals: 0,
    };
    let int = |name: &str| Column {
        name: name.to_owned(),
        field_type: FieldType::Integer,
        length: 4,
        decimals: 0,
    };
    vec![
        text("IP Addr", 30),
        int("Port"),
        text("Multicast Addr", 30),
        text("Server Name", 48),
        text("Serial Number", 16),
        text("Version", 16),
        text("Owner", 36),
        text("Install Date", 16),
        int("User Count"),
        int("Server Type"),
        text("Up Time", 15),
        int("Operations"),
        Column {
            name: "More Info".to_owned(),
            field_type: FieldType::Memo,
            length: 10,
            decimals: 0,
        },
    ]
}

/// One row per discovered address: the address, port, responder and name
/// filled, the detail columns left blank (as `ADS_FS_MULTICAST_ONLY` does).
fn row(found: &Found) -> Vec<Value> {
    let blank = || Value::Text(String::new());
    vec![
        Value::Text(found.ip.to_string()),
        Value::Integer(i128::from(found.port)),
        Value::Text(found.responder.to_string()),
        Value::Text(found.name.clone()),
        blank(),           // Serial Number
        blank(),           // Version
        blank(),           // Owner
        blank(),           // Install Date
        Value::Integer(0), // User Count
        Value::Integer(0), // Server Type
        blank(),           // Up Time
        Value::Integer(0), // Operations
        Value::Null,       // More Info (memo)
    ]
}

/// Runs the discovery and builds the result table, returning its handle. With
/// no servers found it returns an empty table (zero rows), as the `phTable`
/// contract documents — unlike the original engine, which returns the system
/// error `50` with a NULL handle (see `docs/ADS_BUGS.md`, `docs/DIFFERENCES.md`).
pub(crate) fn find_servers() -> Result<Handle, Error> {
    let servers = discover();
    let columns = columns();
    let rows: Vec<Vec<Value>> = servers.iter().map(row).collect();
    let file = tflip_storage::build::build(&columns, &rows).map_err(|_| Error::CorruptTable)?;
    tflip_engine::table::standalone(&LOCAL, "__findservers.adt", file)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real reply captured from a 12.0 evaluation server (two advertised
    /// addresses, both on port 6262, then the server name "ADSSERVER1").
    #[test]
    fn parses_a_real_reply() {
        let reply: &[u8] = &[
            0x01, 0x02, // marker, address count = 2
            0x02, 0x00, 0x18, 0x76, 0xc0, 0xa8, 0x00, 0x0a, 0, 0, 0, 0, 0, 0, 0,
            0, // 192.168.0.10:6262
            0x02, 0x00, 0x18, 0x76, 0xac, 0x11, 0x00, 0x01, 0, 0, 0, 0, 0, 0, 0,
            0, // 172.17.0.1:6262
            0xe7, 0xbd, 0xf7, 0xc4, // server id
            0x00, 0x00, 0x00, 0x01, // flag
            b'A', b'D', b'S', b'S', b'E', b'R', b'V', b'E', b'R', b'1', 0x00,
        ];
        let mut found = Vec::new();
        parse_reply(reply, Ipv4Addr::new(192, 168, 0, 10), &mut found);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].ip, Ipv4Addr::new(192, 168, 0, 10));
        assert_eq!(found[0].port, 6262);
        assert_eq!(found[0].name, "ADSSERVER1");
        assert_eq!(found[0].responder, Ipv4Addr::new(192, 168, 0, 10));
        assert_eq!(found[1].ip, Ipv4Addr::new(172, 17, 0, 1));
        assert_eq!(found[1].port, 6262);
    }

    /// A truncated or foreign datagram is ignored, not a panic.
    #[test]
    fn ignores_junk() {
        let mut found = Vec::new();
        parse_reply(&[], Ipv4Addr::UNSPECIFIED, &mut found);
        parse_reply(&[0x01, 0x05, 0x02, 0x00], Ipv4Addr::UNSPECIFIED, &mut found); // count 5, short
        parse_reply(&[0x02, 0x01], Ipv4Addr::UNSPECIFIED, &mut found); // wrong marker
        assert!(found.is_empty());
    }
}
