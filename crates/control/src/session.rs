//! Một phiên điều khiển: kết nối, bắt tay, rồi nối ô bắt chuột với socket.

use std::io::Write;
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use smartmaupad_shared::handshake::client_handshake;
use smartmaupad_shared::protocol::{
    push_coalesced, Message, CONNECT_TIMEOUT, PING_INTERVAL, SESSION_TIMEOUT,
};

use crate::capture::{self, CaptureControl, StopReason};
use crate::hotkey::Hotkey;

pub struct Params {
    pub host: String,
    pub port: u16,
    pub password: String,
    pub sensitivity: f32,
    pub hotkey: Hotkey,
}

pub enum Event {
    /// Đã xác thực xong, ô bắt chuột sắp mở.
    Connected,
    /// Độ trễ khứ hồi đo từ Ping/Pong.
    Rtt(Duration),
    /// Phiên đã kết thúc: `Ok` là dừng bình thường, `Err` là lỗi.
    Ended(Result<String, String>),
}

pub struct Session {
    control: Arc<CaptureControl>,
    pub events: Receiver<Event>,
}

impl Session {
    pub fn start(params: Params, ctx: egui::Context) -> Self {
        let control = Arc::new(CaptureControl::default());
        let (events_tx, events) = mpsc::channel();
        let notifier = Notifier { tx: events_tx, ctx };
        let thread_control = control.clone();
        thread::spawn(move || {
            let outcome = run(params, &thread_control, &notifier);
            notifier.send(Event::Ended(outcome));
        });
        Self { control, events }
    }

    pub fn cancel(&self) {
        self.control.cancel();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Clone)]
struct Notifier {
    tx: Sender<Event>,
    ctx: egui::Context,
}

impl Notifier {
    fn send(&self, event: Event) {
        let _ = self.tx.send(event);
        self.ctx.request_repaint();
    }
}

/// Seq và thời điểm gửi của Ping gần nhất, để thread đọc tính độ trễ.
type LastPing = Arc<Mutex<Option<(u32, Instant)>>>;

fn run(params: Params, control: &Arc<CaptureControl>, notifier: &Notifier) -> Result<String, String> {
    let Params { host, port, password, sensitivity, hotkey } = params;
    let label = format!("{host}:{port}");

    let addrs: Vec<_> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("Không phân giải được địa chỉ {host}: {e}"))?
        .collect();
    // Target chỉ nghe IPv4 nên ưu tiên địa chỉ IPv4.
    let addr = addrs
        .iter()
        .find(|addr| addr.is_ipv4())
        .or(addrs.first())
        .ok_or_else(|| format!("Không tìm thấy địa chỉ nào cho {host}."))?;

    let mut stream = TcpStream::connect_timeout(addr, CONNECT_TIMEOUT)
        .map_err(|e| format!("Không kết nối được tới {label}: {e}"))?;
    if control.is_stop_requested() {
        return Ok("Đã huỷ.".to_owned());
    }
    stream
        .set_nodelay(true)
        .and_then(|()| stream.set_read_timeout(Some(SESSION_TIMEOUT)))
        .and_then(|()| stream.set_write_timeout(Some(SESSION_TIMEOUT)))
        .map_err(|e| format!("Không cấu hình được kết nối: {e}"))?;
    client_handshake(&mut stream, &password).map_err(|e| format!("Target từ chối: {e}."))?;

    let clone_error = |e| format!("Không nhân bản được socket: {e}");
    let write_stream = stream.try_clone().map_err(clone_error)?;
    let read_stream = stream.try_clone().map_err(clone_error)?;
    notifier.send(Event::Connected);

    let (tx, rx) = mpsc::channel();
    let last_ping = LastPing::default();
    let writer = {
        let (control, last_ping) = (control.clone(), last_ping.clone());
        thread::spawn(move || write_loop(write_stream, rx, sensitivity, &last_ping, &control))
    };
    let reader = {
        let (control, notifier) = (control.clone(), notifier.clone());
        thread::spawn(move || read_loop(read_stream, &last_ping, &control, &notifier))
    };

    // Chặn ở đây cho tới khi ô bắt chuột đóng. `tx` bị drop khi hàm trả về,
    // nhờ đó thread ghi gửi nốt ReleaseAll rồi tự thoát.
    let reason = capture::run(tx, hotkey, &label, control.clone());
    let _ = writer.join();
    let _ = stream.shutdown(Shutdown::Both);
    let _ = reader.join();

    match reason {
        StopReason::Hotkey => Ok(format!("Đã dừng bằng phím tắt {hotkey}.")),
        StopReason::Cancelled => Ok("Đã dừng.".to_owned()),
        StopReason::FocusLost => Ok("Đã dừng vì ô điều khiển mất focus.".to_owned()),
        StopReason::Network => Err(format!("Mất kết nối tới {label}.")),
        StopReason::Failed(error) => Err(error),
    }
}

/// Nhân delta chuột với độ nhạy, giữ lại phần lẻ để chuyển động chậm không bị mất.
struct Scaler {
    factor: f32,
    remainder: (f32, f32),
}

impl Scaler {
    fn new(factor: f32) -> Self {
        Self { factor, remainder: (0.0, 0.0) }
    }

    fn apply(&mut self, dx: i32, dy: i32) -> (i32, i32) {
        let x = dx as f32 * self.factor + self.remainder.0;
        let y = dy as f32 * self.factor + self.remainder.1;
        let (whole_x, whole_y) = (x.trunc(), y.trunc());
        self.remainder = (x - whole_x, y - whole_y);
        (whole_x as i32, whole_y as i32)
    }
}

fn write_loop(
    mut stream: TcpStream,
    rx: Receiver<Message>,
    sensitivity: f32,
    last_ping: &LastPing,
    control: &CaptureControl,
) {
    let mut scaler = Scaler::new(sensitivity);
    let mut batch = Vec::new();
    let mut buf = Vec::new();
    let mut seq = 0u32;
    let mut next_ping = Instant::now();

    loop {
        let mut closing = false;
        match rx.recv_timeout(next_ping.saturating_duration_since(Instant::now())) {
            Ok(first) => {
                push_coalesced(&mut batch, first);
                // Rút hết hàng đợi để các move dồn lại được gộp thành một.
                loop {
                    match rx.try_recv() {
                        Ok(msg) => push_coalesced(&mut batch, msg),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            closing = true;
                            break;
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => closing = true,
        }

        buf.clear();
        for msg in batch.drain(..) {
            match msg {
                Message::MouseMove { dx, dy } => {
                    let (dx, dy) = scaler.apply(dx, dy);
                    if dx != 0 || dy != 0 {
                        Message::MouseMove { dx, dy }.encode(&mut buf);
                    }
                }
                other => other.encode(&mut buf),
            }
        }
        let now = Instant::now();
        if now >= next_ping {
            seq = seq.wrapping_add(1);
            *last_ping.lock().unwrap_or_else(|p| p.into_inner()) = Some((seq, now));
            Message::Ping(seq).encode(&mut buf);
            next_ping = now + PING_INTERVAL;
        }

        if !buf.is_empty() && stream.write_all(&buf).is_err() {
            control.stop_network();
            return;
        }
        if closing {
            return;
        }
    }
}

fn read_loop(mut stream: TcpStream, last_ping: &LastPing, control: &CaptureControl, notifier: &Notifier) {
    loop {
        match Message::read_from(&mut stream) {
            Ok(Message::Pong(seq)) => {
                let sent = *last_ping.lock().unwrap_or_else(|p| p.into_inner());
                if let Some((_, at)) = sent.filter(|(last_seq, _)| *last_seq == seq) {
                    notifier.send(Event::Rtt(at.elapsed()));
                }
            }
            Ok(_) => {}
            // Gồm cả timeout: target trả Pong mỗi giây, im lặng quá
            // SESSION_TIMEOUT nghĩa là đường truyền đã chết.
            Err(_) => {
                control.stop_network();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Scaler;

    #[test]
    fn scaler_keeps_fractional_motion() {
        let mut scaler = Scaler::new(0.25);
        let total: i32 = (0..8).map(|_| scaler.apply(1, -1).0).sum();
        assert_eq!(total, 2);
        let mut scaler = Scaler::new(0.25);
        let total: i32 = (0..8).map(|_| scaler.apply(1, -1).1).sum();
        assert_eq!(total, -2);
    }

    #[test]
    fn scaler_is_identity_at_one() {
        let mut scaler = Scaler::new(1.0);
        assert_eq!(scaler.apply(7, -3), (7, -3));
        assert_eq!(scaler.apply(-120, 4000), (-120, 4000));
    }
}
