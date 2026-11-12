//! Bắt tay xác thực kiểu challenge-response.
//!
//! 1. control gửi `MAGIC + VERSION`.
//! 2. target gửi nonce 32 byte ngẫu nhiên.
//! 3. control gửi `HMAC-SHA256(key = mật khẩu, msg = CONTEXT || nonce)`.
//! 4. target trả một byte trạng thái.
//!
//! Mật khẩu không bao giờ đi qua mạng. Bắt tay chỉ xác thực lúc kết nối, không
//! mã hoá luồng dữ liệu phía sau.

use std::fmt;
use std::io::{self, Read, Write};

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::protocol::{MAGIC, VERSION};

type HmacSha256 = Hmac<Sha256>;

const CONTEXT: &[u8] = b"smartmaupad-auth-v1";
const NONCE_LEN: usize = 32;
const TAG_LEN: usize = 32;

pub const STATUS_BAD_PASSWORD: u8 = 0;
pub const STATUS_OK: u8 = 1;
pub const STATUS_BUSY: u8 = 2;

#[derive(Debug)]
pub enum HandshakeError {
    Io(io::Error),
    /// Bên kia không nói giao thức SmartMaupad hoặc khác phiên bản.
    BadHello,
    BadPassword,
    /// Target đang phục vụ một control khác.
    Busy,
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HandshakeError::Io(e) => write!(f, "lỗi mạng khi bắt tay: {e}"),
            HandshakeError::BadHello => f.write_str("bên kia không phải SmartMaupad cùng phiên bản"),
            HandshakeError::BadPassword => f.write_str("sai mật khẩu"),
            HandshakeError::Busy => f.write_str("target đang được máy khác điều khiển"),
        }
    }
}

impl std::error::Error for HandshakeError {}

impl From<io::Error> for HandshakeError {
    fn from(e: io::Error) -> Self {
        HandshakeError::Io(e)
    }
}

fn mac_for(password: &str, nonce: &[u8; NONCE_LEN]) -> HmacSha256 {
    let mut mac =
        HmacSha256::new_from_slice(password.as_bytes()).expect("HMAC nhận key độ dài bất kỳ");
    mac.update(CONTEXT);
    mac.update(nonce);
    mac
}

/// Phía control: chạy trọn bắt tay và đọc trạng thái target trả về.
pub fn client_handshake<S: Read + Write>(stream: &mut S, password: &str) -> Result<(), HandshakeError> {
    let mut hello = [0u8; 5];
    hello[..4].copy_from_slice(&MAGIC);
    hello[4] = VERSION;
    stream.write_all(&hello)?;

    let mut nonce = [0u8; NONCE_LEN];
    stream.read_exact(&mut nonce)?;
    let tag = mac_for(password, &nonce).finalize().into_bytes();
    stream.write_all(&tag)?;

    let mut status = [0u8; 1];
    stream.read_exact(&mut status)?;
    match status[0] {
        STATUS_OK => Ok(()),
        STATUS_BUSY => Err(HandshakeError::Busy),
        _ => Err(HandshakeError::BadPassword),
    }
}

/// Phía target: kiểm tra hello và mật khẩu. Chưa gửi trạng thái — bên gọi quyết
/// định (ví dụ từ chối vì đang bận) rồi gọi [`server_reply`].
pub fn server_verify<S: Read + Write>(stream: &mut S, password: &str) -> Result<(), HandshakeError> {
    let mut hello = [0u8; 5];
    stream.read_exact(&mut hello)?;
    if hello[..4] != MAGIC || hello[4] != VERSION {
        return Err(HandshakeError::BadHello);
    }

    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce)
        .map_err(|_| io::Error::other("không lấy được số ngẫu nhiên từ hệ điều hành"))?;
    stream.write_all(&nonce)?;

    let mut tag = [0u8; TAG_LEN];
    stream.read_exact(&mut tag)?;
    // verify_slice so sánh constant-time.
    mac_for(password, &nonce)
        .verify_slice(&tag)
        .map_err(|_| HandshakeError::BadPassword)
}

pub fn server_reply<S: Write>(stream: &mut S, status: u8) -> io::Result<()> {
    stream.write_all(&[status])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    /// Chạy một lượt bắt tay qua loopback, trả kết quả của (server, client).
    fn run(
        server_password: &'static str,
        client_password: &'static str,
        ok_status: u8,
    ) -> (Result<(), HandshakeError>, Result<(), HandshakeError>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let verified = server_verify(&mut stream, server_password);
            let status = match &verified {
                Ok(()) => ok_status,
                Err(_) => STATUS_BAD_PASSWORD,
            };
            server_reply(&mut stream, status).unwrap();
            verified
        });
        let mut client = TcpStream::connect(addr).unwrap();
        let client_result = client_handshake(&mut client, client_password);
        (server.join().unwrap(), client_result)
    }

    #[test]
    fn accepts_matching_password() {
        let (server, client) = run("mật khẩu 123", "mật khẩu 123", STATUS_OK);
        assert!(server.is_ok());
        assert!(client.is_ok());
    }

    #[test]
    fn rejects_wrong_password() {
        let (server, client) = run("dung", "sai", STATUS_OK);
        assert!(matches!(server, Err(HandshakeError::BadPassword)));
        assert!(matches!(client, Err(HandshakeError::BadPassword)));
    }

    #[test]
    fn reports_busy_target() {
        let (server, client) = run("abc", "abc", STATUS_BUSY);
        assert!(server.is_ok());
        assert!(matches!(client, Err(HandshakeError::Busy)));
    }

    #[test]
    fn rejects_foreign_protocol() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            server_verify(&mut stream, "abc")
        });
        let mut client = TcpStream::connect(addr).unwrap();
        client.write_all(b"GET /").unwrap();
        assert!(matches!(server.join().unwrap(), Err(HandshakeError::BadHello)));
    }
}
