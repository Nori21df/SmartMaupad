//! Giao thức nhị phân giữa control và target.
//!
//! Mỗi frame là `[type: u8][payload cố định theo type]`, số nguyên little-endian.

use std::io::{self, Read, Write};
use std::time::Duration;

pub const MAGIC: [u8; 4] = *b"SMPD";
pub const VERSION: u8 = 1;
pub const DEFAULT_PORT: u16 = 45800;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const PING_INTERVAL: Duration = Duration::from_secs(1);
/// Không nhận được gì trong khoảng này thì coi như phiên đã chết.
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(5);

/// Giá trị target gắn vào `dwExtraInfo` của input nó bơm, để control nhận ra
/// và bỏ qua khi hai app chạy chung một máy (tránh vòng lặp).
pub const INJECT_TAG: usize = 0x534D_5044;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MouseButton {
    Left = 0,
    Right = 1,
    Middle = 2,
    X1 = 3,
    X2 = 4,
}

impl MouseButton {
    pub const ALL: [MouseButton; 5] = [
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::X1,
        MouseButton::X2,
    ];

    fn from_u8(value: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|b| *b as u8 == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    MouseMove { dx: i32, dy: i32 },
    MouseButton { button: MouseButton, down: bool },
    Wheel { delta: i16, horizontal: bool },
    Key { scan: u16, vk: u16, extended: bool, down: bool },
    /// Nhả mọi phím và nút chuột đang giữ trên target.
    ReleaseAll,
    Ping(u32),
    Pong(u32),
}

const T_MOVE: u8 = 1;
const T_BUTTON: u8 = 2;
const T_WHEEL: u8 = 3;
const T_KEY: u8 = 4;
const T_RELEASE_ALL: u8 = 5;
const T_PING: u8 = 6;
const T_PONG: u8 = 7;

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

fn read_array<const N: usize, R: Read>(reader: &mut R) -> io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

impl Message {
    /// Nối frame của message vào cuối `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match *self {
            Message::MouseMove { dx, dy } => {
                out.push(T_MOVE);
                out.extend_from_slice(&dx.to_le_bytes());
                out.extend_from_slice(&dy.to_le_bytes());
            }
            Message::MouseButton { button, down } => {
                out.extend_from_slice(&[T_BUTTON, button as u8, down as u8]);
            }
            Message::Wheel { delta, horizontal } => {
                out.push(T_WHEEL);
                out.extend_from_slice(&delta.to_le_bytes());
                out.push(horizontal as u8);
            }
            Message::Key { scan, vk, extended, down } => {
                out.push(T_KEY);
                out.extend_from_slice(&scan.to_le_bytes());
                out.extend_from_slice(&vk.to_le_bytes());
                out.extend_from_slice(&[extended as u8, down as u8]);
            }
            Message::ReleaseAll => out.push(T_RELEASE_ALL),
            Message::Ping(seq) => {
                out.push(T_PING);
                out.extend_from_slice(&seq.to_le_bytes());
            }
            Message::Pong(seq) => {
                out.push(T_PONG);
                out.extend_from_slice(&seq.to_le_bytes());
            }
        }
    }

    pub fn write_to<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        let mut buf = Vec::with_capacity(9);
        self.encode(&mut buf);
        writer.write_all(&buf)
    }

    /// Đọc đúng một frame. Trả lỗi `InvalidData` nếu gặp type hoặc giá trị lạ.
    pub fn read_from<R: Read>(reader: &mut R) -> io::Result<Self> {
        let [kind] = read_array::<1, _>(reader)?;
        Ok(match kind {
            T_MOVE => {
                let b = read_array::<8, _>(reader)?;
                Message::MouseMove {
                    dx: i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    dy: i32::from_le_bytes([b[4], b[5], b[6], b[7]]),
                }
            }
            T_BUTTON => {
                let b = read_array::<2, _>(reader)?;
                Message::MouseButton {
                    button: MouseButton::from_u8(b[0]).ok_or_else(|| invalid("nút chuột lạ"))?,
                    down: b[1] != 0,
                }
            }
            T_WHEEL => {
                let b = read_array::<3, _>(reader)?;
                Message::Wheel {
                    delta: i16::from_le_bytes([b[0], b[1]]),
                    horizontal: b[2] != 0,
                }
            }
            T_KEY => {
                let b = read_array::<6, _>(reader)?;
                Message::Key {
                    scan: u16::from_le_bytes([b[0], b[1]]),
                    vk: u16::from_le_bytes([b[2], b[3]]),
                    extended: b[4] != 0,
                    down: b[5] != 0,
                }
            }
            T_RELEASE_ALL => Message::ReleaseAll,
            T_PING => Message::Ping(u32::from_le_bytes(read_array::<4, _>(reader)?)),
            T_PONG => Message::Pong(u32::from_le_bytes(read_array::<4, _>(reader)?)),
            _ => return Err(invalid("loại message lạ")),
        })
    }
}

/// Thêm `msg` vào `batch`, gộp với phần tử cuối nếu cả hai đều là `MouseMove`.
///
/// Chuột 1000Hz sinh rất nhiều move nhỏ; gộp lại giúp mỗi lần ghi socket chỉ
/// mang một move mà không đổi thứ tự so với click hay phím.
pub fn push_coalesced(batch: &mut Vec<Message>, msg: Message) {
    if let (Some(Message::MouseMove { dx, dy }), Message::MouseMove { dx: ndx, dy: ndy }) =
        (batch.last_mut(), msg)
    {
        *dx = dx.saturating_add(ndx);
        *dy = dy.saturating_add(ndy);
    } else {
        batch.push(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Vec<Message> {
        vec![
            Message::MouseMove { dx: -5, dy: 123_456 },
            Message::MouseButton { button: MouseButton::Left, down: true },
            Message::MouseButton { button: MouseButton::X2, down: false },
            Message::Wheel { delta: -120, horizontal: false },
            Message::Wheel { delta: 240, horizontal: true },
            Message::Key { scan: 0x1D, vk: 0xA3, extended: true, down: true },
            Message::Key { scan: 0, vk: 0xB3, extended: false, down: false },
            Message::ReleaseAll,
            Message::Ping(7),
            Message::Pong(u32::MAX),
        ]
    }

    #[test]
    fn round_trip_stream() {
        let mut buf = Vec::new();
        for msg in samples() {
            msg.encode(&mut buf);
        }
        let mut cursor = io::Cursor::new(buf);
        for expected in samples() {
            assert_eq!(Message::read_from(&mut cursor).unwrap(), expected);
        }
        let eof = Message::read_from(&mut cursor).unwrap_err();
        assert_eq!(eof.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn rejects_unknown_type_and_button() {
        let err = Message::read_from(&mut io::Cursor::new([0xEEu8])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let err = Message::read_from(&mut io::Cursor::new([T_BUTTON, 9, 1])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn coalesces_only_adjacent_moves() {
        let click = Message::MouseButton { button: MouseButton::Left, down: true };
        let mut batch = Vec::new();
        for msg in [
            Message::MouseMove { dx: 1, dy: 2 },
            Message::MouseMove { dx: 3, dy: -4 },
            click,
            Message::MouseMove { dx: 10, dy: 10 },
            Message::MouseMove { dx: i32::MAX, dy: 0 },
        ] {
            push_coalesced(&mut batch, msg);
        }
        assert_eq!(
            batch,
            vec![
                Message::MouseMove { dx: 4, dy: -2 },
                click,
                Message::MouseMove { dx: i32::MAX, dy: 10 },
            ]
        );
    }
}
