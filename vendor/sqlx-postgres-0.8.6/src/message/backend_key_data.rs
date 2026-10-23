use byteorder::{BigEndian, ByteOrder};
use sqlx_core::bytes::Bytes;

use crate::error::Error;
use crate::message::{BackendMessage, BackendMessageFormat};

/// Contains cancellation key data. The frontend must save these values if it
/// wishes to be able to issue `CancelRequest` messages later.
pub struct BackendKeyData {
    /// The process ID of this database.
    pub process_id: u32,

    /// The secret key of this database.
    pub secret_key: u32,
}

impl BackendMessage for BackendKeyData {
    const FORMAT: BackendMessageFormat = BackendMessageFormat::BackendKeyData;

    fn decode_body(buf: Bytes) -> Result<Self, Error> {
        // SQLx 0.8.6 negotiates protocol 3.0, not 3.2 variable keys.
        // Reject malformed/unsupported frames rather than truncate a capability.
        if buf.len() != 8 {
            return Err(err_protocol!("unsupported BackendKeyData length"));
        }
        let process_id = BigEndian::read_u32(&buf);
        let secret_key = BigEndian::read_u32(&buf[4..]);

        Ok(Self {
            process_id,
            secret_key,
        })
    }
}

#[test]
fn test_decode_backend_key_data() {
    const DATA: &[u8] = b"\0\0'\xc6\x89R\xc5+";

    let m = BackendKeyData::decode_body(DATA.into()).unwrap();

    assert_eq!(m.process_id, 10182);
    assert_eq!(m.secret_key, 2303903019);
}

#[cfg(all(test, not(debug_assertions)))]
#[bench]
fn bench_decode_backend_key_data(b: &mut test::Bencher) {
    const DATA: &[u8] = b"\0\0'\xc6\x89R\xc5+";

    b.iter(|| {
        BackendKeyData::decode_body(test::black_box(Bytes::from_static(DATA))).unwrap();
    });
}

impl std::fmt::Debug for BackendKeyData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BackendKeyData([REDACTED])")
    }
}
#[test]
fn rejects_unsupported_backend_key_lengths() {
    for bytes in [vec![], vec![0; 7], vec![0; 9], vec![0; 36]] {
        assert!(BackendKeyData::decode_body(bytes.into()).is_err());
    }
}
