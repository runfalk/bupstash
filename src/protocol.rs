use super::address::*;

use super::store;
use serde::{Deserialize, Serialize};
use std::convert::TryInto;

const MAX_PACKET_SIZE: usize = 1024 * 1024 * 16;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct ServerInfo {
    pub protocol: String,
}

#[derive(Debug, PartialEq)]
pub struct Chunk {
    pub address: Address,
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct BeginSend {}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct AckSend {
    pub gc_generation: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct CommitSend {
    pub address: Address,
    pub metadata: store::ItemMetadata,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct AckCommit {}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct RequestData {
    pub root: Address,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct AckRequestData {
    pub metadata: Option<store::ItemMetadata>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct StartGC {}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct GCComplete {
    pub stats: store::GCStats,
}

#[derive(Debug, PartialEq)]
pub enum Packet {
    ServerInfo(ServerInfo),
    BeginSend(BeginSend),
    AckSend(AckSend),
    Chunk(Chunk),
    CommitSend(CommitSend),
    AckCommit(AckCommit),
    RequestData(RequestData),
    AckRequestData(AckRequestData),
    StartGC(StartGC),
    GCComplete(GCComplete),
}

const PACKET_KIND_SERVER_INFO: u8 = 0;
const PACKET_KIND_BEGIN_SEND: u8 = 1;
const PACKET_KIND_ACK_SEND: u8 = 2;
const PACKET_KIND_CHUNK: u8 = 3;
const PACKET_KIND_COMMIT_SEND: u8 = 4;
const PACKET_KIND_ACK_COMMIT: u8 = 5;
const PACKET_KIND_REQUEST_DATA: u8 = 6;
const PACKET_KIND_ACK_REQUEST_DATA: u8 = 7;
const PACKET_KIND_START_GC: u8 = 8;
const PACKET_KIND_GC_COMPLETE: u8 = 9;

fn read_from_remote(r: &mut dyn std::io::Read, buf: &mut [u8]) -> Result<(), failure::Error> {
    if let Err(_) = r.read_exact(buf) {
        failure::bail!("repository disconnected");
    };
    Ok(())
}

pub fn read_packet(r: &mut dyn std::io::Read) -> Result<Packet, failure::Error> {
    let mut hdr: [u8; 5] = [0; 5];
    read_from_remote(r, &mut hdr[..])?;
    let sz = (hdr[0] as usize) << 24
        | (hdr[1] as usize) << 16
        | (hdr[2] as usize) << 8
        | (hdr[3] as usize);

    if sz > MAX_PACKET_SIZE {
        failure::bail!("packet too large");
    }

    let kind = hdr[4];

    let mut buf: Vec<u8> = Vec::with_capacity(sz);
    // We just created buf with capacity sz and u8 is a primitive type.
    // This means we don't need to write the buffer memory twice.
    unsafe {
        buf.set_len(sz);
    };
    read_from_remote(r, &mut buf)?;
    let packet = match kind {
        PACKET_KIND_SERVER_INFO => Packet::ServerInfo(serde_json::from_slice(&buf)?),
        PACKET_KIND_BEGIN_SEND => Packet::BeginSend(serde_json::from_slice(&buf)?),
        PACKET_KIND_ACK_SEND => Packet::AckSend(serde_json::from_slice(&buf)?),
        PACKET_KIND_CHUNK => {
            if buf.len() < ADDRESS_SZ {
                failure::bail!("protocol error, chunk smaller than address");
            }

            let mut address = Address { bytes: [0; 32] };

            address.bytes[..].clone_from_slice(&buf[buf.len() - ADDRESS_SZ..]);
            buf.truncate(buf.len() - ADDRESS_SZ);
            Packet::Chunk(Chunk { address, data: buf })
        }
        PACKET_KIND_COMMIT_SEND => Packet::CommitSend(serde_json::from_slice(&buf)?),
        PACKET_KIND_ACK_COMMIT => Packet::AckCommit(serde_json::from_slice(&buf)?),
        PACKET_KIND_REQUEST_DATA => Packet::RequestData(serde_json::from_slice(&buf)?),
        PACKET_KIND_ACK_REQUEST_DATA => Packet::AckRequestData(serde_json::from_slice(&buf)?),
        PACKET_KIND_START_GC => Packet::StartGC(serde_json::from_slice(&buf)?),
        PACKET_KIND_GC_COMPLETE => Packet::GCComplete(serde_json::from_slice(&buf)?),
        _ => return Err(failure::format_err!("protocol error, unknown packet kind")),
    };
    Ok(packet)
}

fn send_hdr(w: &mut dyn std::io::Write, kind: u8, sz: u32) -> Result<(), failure::Error> {
    let mut hdr: [u8; 5] = [0; 5];
    hdr[0] = ((sz & 0xff00_0000) >> 24) as u8;
    hdr[1] = ((sz & 0x00ff_0000) >> 16) as u8;
    hdr[2] = ((sz & 0x0000_ff00) >> 8) as u8;
    hdr[3] = (sz & 0x0000_00ff) as u8;
    hdr[4] = kind;
    w.write_all(&hdr[..])?;
    Ok(())
}

pub fn write_packet(w: &mut dyn std::io::Write, pkt: &Packet) -> Result<(), failure::Error> {
    match pkt {
        Packet::Chunk(ref v) => {
            send_hdr(
                w,
                PACKET_KIND_CHUNK,
                (v.data.len() + ADDRESS_SZ).try_into()?,
            )?;
            w.write_all(&v.data)?;
            w.write_all(&v.address.bytes)?;
        }
        // XXX Refactor somehow. Generic, macro?
        // Only the chunk packet needs special treatment.
        Packet::ServerInfo(ref v) => {
            let j = serde_json::to_string(&v)?;
            let b = j.as_bytes();
            send_hdr(w, PACKET_KIND_SERVER_INFO, b.len().try_into()?)?;
            w.write_all(b)?;
        }
        Packet::BeginSend(ref v) => {
            let j = serde_json::to_string(&v)?;
            let b = j.as_bytes();
            send_hdr(w, PACKET_KIND_BEGIN_SEND, b.len().try_into()?)?;
            w.write_all(b)?;
        }
        Packet::AckSend(ref v) => {
            let j = serde_json::to_string(&v)?;
            let b = j.as_bytes();
            send_hdr(w, PACKET_KIND_ACK_SEND, b.len().try_into()?)?;
            w.write_all(b)?;
        }
        Packet::CommitSend(ref v) => {
            let j = serde_json::to_string(&v)?;
            let b = j.as_bytes();
            send_hdr(w, PACKET_KIND_COMMIT_SEND, b.len().try_into()?)?;
            w.write_all(b)?;
        }
        Packet::AckCommit(ref v) => {
            let j = serde_json::to_string(&v)?;
            let b = j.as_bytes();
            send_hdr(w, PACKET_KIND_ACK_COMMIT, b.len().try_into()?)?;
            w.write_all(b)?;
        }
        Packet::RequestData(ref v) => {
            let j = serde_json::to_string(&v)?;
            let b = j.as_bytes();
            send_hdr(w, PACKET_KIND_REQUEST_DATA, b.len().try_into()?)?;
            w.write_all(b)?;
        }
        Packet::AckRequestData(ref v) => {
            let j = serde_json::to_string(&v)?;
            let b = j.as_bytes();
            send_hdr(w, PACKET_KIND_ACK_REQUEST_DATA, b.len().try_into()?)?;
            w.write_all(b)?;
        }
        Packet::StartGC(ref v) => {
            let j = serde_json::to_string(&v)?;
            let b = j.as_bytes();
            send_hdr(w, PACKET_KIND_START_GC, b.len().try_into()?)?;
            w.write_all(b)?;
        }
        Packet::GCComplete(ref v) => {
            let j = serde_json::to_string(&v)?;
            let b = j.as_bytes();
            send_hdr(w, PACKET_KIND_GC_COMPLETE, b.len().try_into()?)?;
            w.write_all(b)?;
        }
    }
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::crypto;
    use super::super::keys;
    use super::*;

    #[test]
    fn send_recv() {
        let packets = vec![
            Packet::ServerInfo(ServerInfo {
                protocol: "foobar".to_owned(),
            }),
            Packet::BeginSend(BeginSend {}),
            Packet::AckSend(AckSend {
                gc_generation: "blah".to_owned(),
            }),
            Packet::CommitSend(CommitSend {
                address: Address::default(),
                metadata: store::ItemMetadata {
                    tree_height: 3,
                    encrypt_header: {
                        let master_key = keys::MasterKey::gen();
                        let ectx = crypto::EncryptContext::new(&keys::Key::MasterKeyV1(master_key));
                        ectx.encryption_header()
                    },
                },
            }),
            Packet::Chunk(Chunk {
                address: Address::default(),
                data: vec![1, 2, 3],
            }),
            Packet::RequestData(RequestData {
                root: Address::default(),
            }),
            Packet::AckRequestData(AckRequestData {
                metadata: {
                    let master_key = keys::MasterKey::gen();
                    let ectx = crypto::EncryptContext::new(&keys::Key::MasterKeyV1(master_key));
                    Some(store::ItemMetadata {
                        tree_height: 1234,
                        encrypt_header: ectx.encryption_header(),
                    })
                },
            }),
            Packet::StartGC(StartGC {}),
            Packet::GCComplete(GCComplete {
                stats: store::GCStats {
                    chunks_deleted: 123,
                    bytes_freed: 345,
                    bytes_remaining: 678,
                },
            }),
        ];

        for p1 in packets.iter() {
            let mut c1 = std::io::Cursor::new(Vec::new());
            write_packet(&mut c1, p1).unwrap();
            let b = c1.into_inner();
            let mut c2 = std::io::Cursor::new(b);
            let p2 = read_packet(&mut c2).unwrap();
            assert!(p1 == &p2);
        }
    }
}
