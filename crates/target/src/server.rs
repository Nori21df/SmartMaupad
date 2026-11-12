//! Lắng nghe TCP, xác thực control và chuyển message nhận được thành input.

use std::collections::VecDeque;
use std::io::{self, BufReader};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use eframe::egui;
use smartmaupad_shared::handshake::{
    server_reply, server_verify, HandshakeError, STATUS_BAD_PASSWORD, STATUS_BUSY, STATUS_OK,
};
use smartmaupad_shared::protocol::{Message, SESSION_TIMEOUT};

use crate::inject::Injector;

const LOG_CAPACITY: usize = 200;
/// Số kết nối (kể cả đang bắt tay) được xử lý cùng lúc.
const MAX_CONNECTIONS: usize = 8;
const ACCEPT_POLL: Duration = Duration::from_millis(50);
/// Chờ trước khi báo sai mật khẩu để làm chậm việc dò.
const BAD_PASSWORD_DELAY: Duration = Duration::from_secs(1);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct EventLog {
    lines: VecDeque<String>,
    /// Tổng delta của chuỗi move liên tiếp đang nằm ở dòng cuối.
    move_run: Option<(i64, i64)>,
}

impl EventLog {
    fn push(&mut self, line: String) {
        self.move_run = None;
        self.push_line(line);
    }

    /// Gộp các move liên tiếp vào một dòng để log không bị trôi.
    fn push_move(&mut self, dx: i32, dy: i32) {
        let (mut sx, mut sy) = (i64::from(dx), i64::from(dy));
        if let Some((px, py)) = self.move_run {
            sx += px;
            sy += py;
            self.lines.pop_back();
        }
        self.move_run = Some((sx, sy));
        self.push_line(format!("di chuyển  dx={sx} dy={sy}"));
    }

    fn push_line(&mut self, line: String) {
        if self.lines.len() == LOG_CAPACITY {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }
}

/// Trạng thái chia sẻ giữa UI và các thread mạng.
pub struct Shared {
    log: Mutex<EventLog>,
    peer: Mutex<Option<SocketAddr>>,
    /// Bật thì chỉ ghi log, không bơm input.
    pub log_only: AtomicBool,
    ctx: egui::Context,
}

impl Shared {
    pub fn new(ctx: egui::Context, log_only: bool) -> Arc<Self> {
        Arc::new(Self {
            log: Mutex::default(),
            peer: Mutex::default(),
            log_only: AtomicBool::new(log_only),
            ctx,
        })
    }

    pub fn log(&self, line: impl Into<String>) {
        lock(&self.log).push(line.into());
        self.ctx.request_repaint();
    }

    pub fn log_lines(&self) -> Vec<String> {
        lock(&self.log).lines.iter().cloned().collect()
    }

    pub fn peer(&self) -> Option<SocketAddr> {
        *lock(&self.peer)
    }

    fn set_peer(&self, peer: Option<SocketAddr>) {
        *lock(&self.peer) = peer;
        self.ctx.request_repaint();
    }

    fn log_message(&self, msg: &Message) {
        let line = match *msg {
            Message::MouseMove { dx, dy } => {
                lock(&self.log).push_move(dx, dy);
                self.ctx.request_repaint();
                return;
            }
            Message::MouseButton { button, down } => {
                format!("nút chuột  {button:?} {}", if down { "nhấn" } else { "nhả" })
            }
            Message::Wheel { delta, horizontal } => {
                format!("cuộn {}  {delta}", if horizontal { "ngang" } else { "dọc" })
            }
            Message::Key { scan, vk, extended, down } => format!(
                "phím  vk=0x{vk:02X} scan=0x{scan:02X}{} {}",
                if extended { " ext" } else { "" },
                if down { "nhấn" } else { "nhả" }
            ),
            Message::ReleaseAll => "nhả tất cả".to_owned(),
            Message::Ping(_) | Message::Pong(_) => return,
        };
        self.log(line);
    }
}

/// Server đang chạy. Drop hoặc gọi [`Server::stop`] để đóng cổng và ngắt phiên.
pub struct Server {
    stop: Arc<AtomicBool>,
    active: Arc<Mutex<Option<TcpStream>>>,
    thread: Option<JoinHandle<()>>,
}

struct Context {
    password: String,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    /// Bản sao socket của phiên đang chạy, để `stop` có thể ngắt nó.
    active: Arc<Mutex<Option<TcpStream>>>,
    busy: AtomicBool,
    connections: AtomicUsize,
}

impl Server {
    /// Nghe trên mọi card mạng IPv4 ở cổng `port`.
    pub fn start(port: u16, password: String, shared: Arc<Shared>) -> io::Result<Self> {
        Self::bind((Ipv4Addr::UNSPECIFIED, port).into(), password, shared)
    }

    fn bind(addr: SocketAddr, password: String, shared: Arc<Shared>) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;

        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(Mutex::new(None));
        let ctx = Arc::new(Context {
            password,
            shared,
            stop: stop.clone(),
            active: active.clone(),
            busy: AtomicBool::new(false),
            connections: AtomicUsize::new(0),
        });
        let thread = thread::spawn(move || accept_loop(listener, ctx));
        Ok(Self { stop, active, thread: Some(thread) })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(stream) = lock(&self.active).as_ref() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(listener: TcpListener, ctx: Arc<Context>) {
    while !ctx.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if ctx.connections.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    continue;
                }
                ctx.connections.fetch_add(1, Ordering::SeqCst);
                let ctx = ctx.clone();
                thread::spawn(move || {
                    handle_connection(stream, peer, &ctx);
                    ctx.connections.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(_) => thread::sleep(ACCEPT_POLL),
        }
    }
}

fn handle_connection(mut stream: TcpStream, peer: SocketAddr, ctx: &Context) {
    // Trên Windows socket từ accept() thừa hưởng chế độ non-blocking của listener.
    let prepared = stream
        .set_nonblocking(false)
        .and_then(|()| stream.set_nodelay(true))
        .and_then(|()| stream.set_read_timeout(Some(SESSION_TIMEOUT)))
        .and_then(|()| stream.set_write_timeout(Some(SESSION_TIMEOUT)));
    if prepared.is_err() {
        return;
    }

    match server_verify(&mut stream, &ctx.password) {
        Ok(()) => {}
        Err(HandshakeError::BadPassword) => {
            ctx.shared.log(format!("{peer}: sai mật khẩu, đã từ chối"));
            thread::sleep(BAD_PASSWORD_DELAY);
            let _ = server_reply(&mut stream, STATUS_BAD_PASSWORD);
            return;
        }
        // Không phải SmartMaupad (ví dụ máy quét cổng) hoặc rớt giữa chừng.
        Err(_) => return,
    }

    if ctx.busy.swap(true, Ordering::SeqCst) {
        ctx.shared.log(format!("{peer}: từ chối vì đang có phiên khác"));
        let _ = server_reply(&mut stream, STATUS_BUSY);
        return;
    }

    ctx.shared.log(format!("{peer}: đã kết nối"));
    ctx.shared.set_peer(Some(peer));
    run_session(stream, ctx);
    ctx.shared.set_peer(None);
    ctx.shared.log(format!("{peer}: đã ngắt"));
    ctx.busy.store(false, Ordering::SeqCst);
}

fn run_session(mut stream: TcpStream, ctx: &Context) {
    let mut injector = Injector::new();
    if let Ok(reader) = stream.try_clone() {
        *lock(&ctx.active) = stream.try_clone().ok();
        if server_reply(&mut stream, STATUS_OK).is_ok() {
            pump(BufReader::new(reader), &mut stream, &mut injector, ctx);
        }
        *lock(&ctx.active) = None;
    }
    // Dù phiên kết thúc kiểu gì cũng không để lại phím/nút bị kẹt.
    injector.release_all();
}

fn pump(mut reader: BufReader<TcpStream>, writer: &mut TcpStream, injector: &mut Injector, ctx: &Context) {
    while !ctx.stop.load(Ordering::SeqCst) {
        // Lỗi đọc gồm cả timeout: control gửi Ping mỗi giây nên im lặng quá
        // SESSION_TIMEOUT nghĩa là đường truyền đã chết.
        let Ok(msg) = Message::read_from(&mut reader) else {
            return;
        };
        match msg {
            Message::Ping(seq) => {
                if Message::Pong(seq).write_to(writer).is_err() {
                    return;
                }
            }
            Message::Pong(_) => {}
            msg => {
                if ctx.shared.log_only.load(Ordering::Relaxed) {
                    // Vừa chuyển sang chế độ log giữa phiên thì nhả những gì đang giữ.
                    injector.release_all();
                    ctx.shared.log_message(&msg);
                } else {
                    injector.apply(&msg);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smartmaupad_shared::handshake::client_handshake;
    use smartmaupad_shared::protocol::MouseButton;
    use std::time::Instant;

    const PASSWORD: &str = "bí-mật";

    /// Server thật trên loopback, ở chế độ chỉ ghi log nên không bơm input nào.
    fn start() -> (Server, Arc<Shared>, SocketAddr) {
        let shared = Shared::new(egui::Context::default(), true);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let server = Server::bind(addr, PASSWORD.to_owned(), shared.clone()).unwrap();
        (server, shared, addr)
    }

    fn connect(addr: SocketAddr) -> TcpStream {
        let stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream
    }

    fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "hết giờ chờ: {what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn session_handles_events_ping_busy_and_stop() {
        let (mut server, shared, addr) = start();
        let mut stream = connect(addr);
        client_handshake(&mut stream, PASSWORD).unwrap();

        for msg in [
            Message::MouseMove { dx: 3, dy: 4 },
            Message::MouseMove { dx: 1, dy: 1 },
            Message::MouseButton { button: MouseButton::Left, down: true },
            Message::Key { scan: 0x1E, vk: 0x41, extended: false, down: true },
            Message::Ping(42),
        ] {
            msg.write_to(&mut stream).unwrap();
        }
        // Message được xử lý đúng thứ tự nên khi Pong về thì log đã đủ.
        assert_eq!(Message::read_from(&mut stream).unwrap(), Message::Pong(42));
        let lines = shared.log_lines();
        assert!(lines.iter().any(|l| l.contains("dx=4 dy=5")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("Left nhấn")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("vk=0x41")), "{lines:?}");
        assert_eq!(shared.peer().map(|p| p.ip()), Some(addr.ip()));

        // Control thứ hai đúng mật khẩu vẫn bị từ chối vì đang có phiên.
        let mut second = connect(addr);
        assert!(matches!(client_handshake(&mut second, PASSWORD), Err(HandshakeError::Busy)));

        // Control ngắt thì target sẵn sàng nhận phiên mới.
        drop(stream);
        wait_until("phiên cũ kết thúc", || shared.peer().is_none());
        let mut third = connect(addr);
        client_handshake(&mut third, PASSWORD).unwrap();
        wait_until("phiên mới bắt đầu", || shared.peer().is_some());

        // Tắt nghe thì phiên đang chạy bị ngắt và cổng đóng lại.
        server.stop();
        assert!(Message::read_from(&mut third).is_err());
        wait_until("phiên bị ngắt khi tắt nghe", || shared.peer().is_none());
        assert!(TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_err());
    }

    #[test]
    fn wrong_password_is_rejected_without_session() {
        let (_server, shared, addr) = start();
        let mut stream = connect(addr);
        let started = Instant::now();
        assert!(matches!(
            client_handshake(&mut stream, "sai mật khẩu"),
            Err(HandshakeError::BadPassword)
        ));
        assert!(started.elapsed() >= BAD_PASSWORD_DELAY);
        assert!(shared.peer().is_none());
        assert!(shared.log_lines().iter().any(|l| l.contains("sai mật khẩu")));
    }
}
