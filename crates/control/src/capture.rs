//! Ô bắt chuột 200x200.
//!
//! Chạy trên một thread riêng có message loop Win32. Khi ô này mở:
//! - con trỏ bị khoá trong ô bằng `ClipCursor`;
//! - chuột được đọc qua Raw Input nên vẫn có delta khi con trỏ đã chạm mép ô;
//! - bàn phím bị chặn bằng low-level hook và chuyển sang target;
//! - phím tắt dừng được bắt ngay trong hook, đây là cách duy nhất để thoát
//!   bằng tay vì chuột không ra khỏi ô được.

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use smartmaupad_shared::protocol::{Message, MouseButton, INJECT_TAG};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint,
    FillRect, SelectObject, SetBkMode, SetTextColor, HBRUSH, HFONT, PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, ClipCursor, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetClientRect, GetForegroundWindow, GetMessageW, GetSystemMetrics, KillTimer, LoadCursorW,
    PostMessageW, PostQuitMessage, RegisterClassW, SetForegroundWindow, SetTimer,
    SetWindowsHookExW, ShowWindow, UnhookWindowsHookEx, HHOOK, IDC_ARROW, KBDLLHOOKSTRUCT, MSG,
    SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SW_SHOW, WH_KEYBOARD_LL,
    WM_ACTIVATE, WM_APP, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_INPUT, WM_PAINT, WM_TIMER,
    WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::hotkey::Hotkey;

/// Cạnh của ô bắt chuột ở 96 DPI.
const BOX_SIZE: i32 = 200;
const TIMER_ID: usize = 1;
const TIMER_MS: u32 = 250;
/// Số lần timer chờ ô lấy được focus lúc mới mở trước khi bỏ cuộc.
const FOCUS_GRACE_TICKS: u32 = 4;
const WM_APP_STOP: u32 = WM_APP + 1;

// Nền tối và màu accent của Windows 11, dạng COLORREF (0x00BBGGRR).
const COLOR_BACKGROUND: u32 = 0x0020_2020;
const COLOR_BORDER: u32 = 0x00FF_CD60;
const COLOR_TEXT: u32 = 0x00FF_FFFF;

const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWA_BORDER_COLOR: u32 = 34;
const DWMWCP_ROUND: u32 = 2;

const HC_ACTION: i32 = 0;
const LLKHF_EXTENDED: u32 = 0x01;
const LLKHF_INJECTED: u32 = 0x10;
const LLKHF_UP: u32 = 0x80;

const RID_INPUT: u32 = 0x1000_0003;
const RIM_TYPEMOUSE: u32 = 0;
const RIDEV_REMOVE: u32 = 0x1;
const HID_USAGE_PAGE_GENERIC: u16 = 0x01;
const HID_USAGE_GENERIC_MOUSE: u16 = 0x02;
const MOUSE_MOVE_ABSOLUTE: u32 = 0x01;
const MOUSE_VIRTUAL_DESKTOP: u32 = 0x02;
const RI_MOUSE_WHEEL: u32 = 0x0400;
const RI_MOUSE_HWHEEL: u32 = 0x0800;

/// (cờ nhấn, cờ nhả, nút) trong `usButtonFlags` của Raw Input.
const BUTTON_FLAGS: [(u32, u32, MouseButton); 5] = [
    (0x0001, 0x0002, MouseButton::Left),
    (0x0004, 0x0008, MouseButton::Right),
    (0x0010, 0x0020, MouseButton::Middle),
    (0x0040, 0x0080, MouseButton::X1),
    (0x0100, 0x0200, MouseButton::X2),
];

const DT_CENTER: u32 = 0x0001;
const DT_WORDBREAK: u32 = 0x0010;
const DT_CALCRECT: u32 = 0x0400;
const DT_NOPREFIX: u32 = 0x0800;
const BK_TRANSPARENT: i32 = 1;

const MOD_LCTRL: u8 = 1 << 0;
const MOD_RCTRL: u8 = 1 << 1;
const MOD_LALT: u8 = 1 << 2;
const MOD_RALT: u8 = 1 << 3;
const MOD_LSHIFT: u8 = 1 << 4;
const MOD_RSHIFT: u8 = 1 << 5;

const STOP_NONE: u8 = 0;
const STOP_NETWORK: u8 = 1;
const STOP_CANCELLED: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// Người dùng bấm phím tắt dừng.
    Hotkey,
    /// Ô mất focus (Ctrl+Alt+Del, UAC, app khác giành focus).
    FocusLost,
    /// Thread mạng báo mất kết nối.
    Network,
    /// App yêu cầu dừng (ví dụ đang thoát).
    Cancelled,
    /// Không dựng được ô bắt chuột.
    Failed(String),
}

/// Tay cầm để thread khác yêu cầu dừng ô bắt chuột.
#[derive(Default)]
pub struct CaptureControl {
    hwnd: AtomicIsize,
    stop: AtomicU8,
}

impl CaptureControl {
    pub fn stop_network(&self) {
        self.request(STOP_NETWORK);
    }

    pub fn cancel(&self) {
        self.request(STOP_CANCELLED);
    }

    pub fn is_stop_requested(&self) -> bool {
        self.stop.load(Ordering::SeqCst) != STOP_NONE
    }

    fn request(&self, code: u8) {
        // Giữ lý do đầu tiên.
        let _ = self.stop.compare_exchange(STOP_NONE, code, Ordering::SeqCst, Ordering::SeqCst);
        let hwnd = self.hwnd.load(Ordering::SeqCst);
        if hwnd != 0 {
            // SAFETY: PostMessageW an toàn với mọi giá trị HWND; cửa sổ đã huỷ thì chỉ trả lỗi.
            unsafe {
                PostMessageW(hwnd as HWND, WM_APP_STOP, 0, 0);
            }
        }
    }

    fn requested_reason(&self) -> Option<StopReason> {
        match self.stop.load(Ordering::SeqCst) {
            STOP_NONE => None,
            STOP_NETWORK => Some(StopReason::Network),
            _ => Some(StopReason::Cancelled),
        }
    }
}

struct State {
    tx: Sender<Message>,
    hotkey: Hotkey,
    control: Arc<CaptureControl>,
    hook: HHOOK,
    font: HFONT,
    background: HBRUSH,
    border: HBRUSH,
    text: Vec<u16>,
    reason: Option<StopReason>,
    /// Bit `MOD_*` của các phím bổ trợ đang được giữ thật trên bàn phím.
    mods: u8,
    /// Nút chuột mà ta đã gửi "nhấn", để không gửi "nhả" mồ côi (ví dụ lần nhả
    /// của cú click vào nút Bắt đầu).
    buttons_down: u8,
    last_abs: Option<(i32, i32)>,
    activated: bool,
    unfocused_ticks: u32,
}

enum KeyOutcome {
    Swallow,
    Stop,
}

impl State {
    fn send(&self, msg: Message) {
        // Thread ghi đã thoát nghĩa là phiên đang đóng; bỏ qua là đúng.
        let _ = self.tx.send(msg);
    }

    fn on_key(&mut self, vk: u16, scan: u16, extended: bool, down: bool) -> KeyOutcome {
        let bit = match vk {
            0x11 | 0xA2 => MOD_LCTRL,
            0xA3 => MOD_RCTRL,
            0x12 | 0xA4 => MOD_LALT,
            0xA5 => MOD_RALT,
            0x10 | 0xA0 => MOD_LSHIFT,
            0xA1 => MOD_RSHIFT,
            _ => 0,
        };
        if down {
            self.mods |= bit;
        } else {
            self.mods &= !bit;
        }

        if self.reason.is_some() {
            return KeyOutcome::Swallow;
        }
        if down && vk == self.hotkey.vk && self.mods_match_hotkey() {
            self.reason = Some(StopReason::Hotkey);
            return KeyOutcome::Stop;
        }
        self.send(Message::Key { scan, vk, extended, down });
        KeyOutcome::Swallow
    }

    fn mods_match_hotkey(&self) -> bool {
        let held = |mask: u8| self.mods & mask != 0;
        held(MOD_LCTRL | MOD_RCTRL) == self.hotkey.ctrl
            && held(MOD_LALT | MOD_RALT) == self.hotkey.alt
            && held(MOD_LSHIFT | MOD_RSHIFT) == self.hotkey.shift
    }

    fn on_mouse(&mut self, flags: u32, button_flags: u32, button_data: u16, x: i32, y: i32) {
        if self.reason.is_some() {
            return;
        }

        let (dx, dy) = if flags & MOUSE_MOVE_ABSOLUTE != 0 {
            // Tablet, máy ảo, RDP báo toạ độ tuyệt đối chuẩn hoá 0..65535.
            let (cx, cy) = if flags & MOUSE_VIRTUAL_DESKTOP != 0 {
                (SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN)
            } else {
                (SM_CXSCREEN, SM_CYSCREEN)
            };
            // SAFETY: GetSystemMetrics không có tiền điều kiện.
            let (w, h) = unsafe { (GetSystemMetrics(cx), GetSystemMetrics(cy)) };
            let px = (i64::from(x) * i64::from(w) / 65535) as i32;
            let py = (i64::from(y) * i64::from(h) / 65535) as i32;
            let delta = self.last_abs.map_or((0, 0), |(lx, ly)| (px - lx, py - ly));
            self.last_abs = Some((px, py));
            delta
        } else {
            (x, y)
        };
        if dx != 0 || dy != 0 {
            self.send(Message::MouseMove { dx, dy });
        }

        for (down_flag, up_flag, button) in BUTTON_FLAGS {
            let bit = 1u8 << (button as u8);
            if button_flags & down_flag != 0 {
                self.buttons_down |= bit;
                self.send(Message::MouseButton { button, down: true });
            }
            if button_flags & up_flag != 0 && self.buttons_down & bit != 0 {
                self.buttons_down &= !bit;
                self.send(Message::MouseButton { button, down: false });
            }
        }

        if button_flags & RI_MOUSE_WHEEL != 0 {
            self.send(Message::Wheel { delta: button_data as i16, horizontal: false });
        }
        if button_flags & RI_MOUSE_HWHEEL != 0 {
            self.send(Message::Wheel { delta: button_data as i16, horizontal: true });
        }
    }
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Chạy `f` với state của thread này. Trả `None` nếu chưa có state hoặc state
/// đang được mượn (callback Win32 lồng nhau) — khi đó bỏ qua thay vì panic
/// xuyên qua ranh giới FFI.
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|cell| cell.try_borrow_mut().ok().and_then(|mut state| state.as_mut().map(f)))
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Mở ô bắt chuột và chặn cho tới khi nó đóng. Sự kiện được đẩy vào `tx`;
/// trước khi trả về luôn gửi thêm `ReleaseAll`.
pub fn run(
    tx: Sender<Message>,
    hotkey: Hotkey,
    target_label: &str,
    control: Arc<CaptureControl>,
) -> StopReason {
    let release_tx = tx.clone();
    // SAFETY: mọi lời gọi Win32 bên trong dùng handle do chính thread này tạo
    // và còn sống cho tới khi dọn dẹp ở cuối hàm.
    let reason = unsafe { run_window(tx, hotkey, target_label, &control) };
    control.hwnd.store(0, Ordering::SeqCst);
    let _ = release_tx.send(Message::ReleaseAll);
    reason
}

unsafe fn run_window(
    tx: Sender<Message>,
    hotkey: Hotkey,
    target_label: &str,
    control: &Arc<CaptureControl>,
) -> StopReason {
    let instance = GetModuleHandleW(null());
    let class_name = wide("SmartMaupadCapture");
    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: null_mut(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class_name.as_ptr(),
    };
    // Lần chạy thứ hai class đã tồn tại và lời gọi này thất bại vô hại.
    RegisterClassW(&class);

    let dpi = GetDpiForSystem().max(96) as i32;
    let size = BOX_SIZE * dpi / 96;
    let x = (GetSystemMetrics(SM_CXSCREEN) - size) / 2;
    let y = (GetSystemMetrics(SM_CYSCREEN) - size) / 2;
    let title = wide("SmartMaupad");
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        class_name.as_ptr(),
        title.as_ptr(),
        WS_POPUP,
        x,
        y,
        size,
        size,
        null_mut(),
        null_mut(),
        instance,
        null(),
    );
    if hwnd.is_null() {
        return StopReason::Failed("Không tạo được ô bắt chuột.".to_owned());
    }

    // Bo góc và viền accent kiểu Windows 11. Trên Windows 10 hai lời gọi này
    // thất bại vô hại và ô giữ viền vuông tự vẽ.
    for (attribute, value) in
        [(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND), (DWMWA_BORDER_COLOR, COLOR_BORDER)]
    {
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            (&value as *const u32).cast::<c_void>(),
            size_of::<u32>() as u32,
        );
    }

    let face = wide("Segoe UI");
    let font = CreateFontW(-(15 * dpi / 96), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr());
    let text = format!("Đang điều khiển\n{target_label}\n\nNhấn {hotkey}\nđể dừng");
    STATE.with(|cell| {
        *cell.borrow_mut() = Some(State {
            tx,
            hotkey,
            control: control.clone(),
            hook: null_mut(),
            font,
            background: CreateSolidBrush(COLOR_BACKGROUND),
            border: CreateSolidBrush(COLOR_BORDER),
            text: text.encode_utf16().collect(),
            reason: None,
            mods: 0,
            buttons_down: 0,
            last_abs: None,
            activated: false,
            unfocused_ticks: 0,
        });
    });

    let mut setup_error = None;
    let mouse = RAWINPUTDEVICE {
        usUsagePage: HID_USAGE_PAGE_GENERIC,
        usUsage: HID_USAGE_GENERIC_MOUSE,
        dwFlags: 0,
        hwndTarget: hwnd,
    };
    if RegisterRawInputDevices(&mouse, 1, size_of::<RAWINPUTDEVICE>() as u32) == 0 {
        setup_error = Some("Không đăng ký được Raw Input cho chuột.");
    }
    let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), instance, 0);
    if hook.is_null() {
        // Không có hook thì không có phím tắt dừng: tuyệt đối không khoá chuột.
        setup_error = Some("Không cài được hook bàn phím.");
    }
    with_state(|state| state.hook = hook);

    if let Some(error) = setup_error {
        with_state(|state| state.reason = Some(StopReason::Failed(error.to_owned())));
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
    } else {
        control.hwnd.store(hwnd as isize, Ordering::SeqCst);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        clip_cursor_to(hwnd);
        SetTimer(hwnd, TIMER_ID, TIMER_MS, None);
        // Yêu cầu dừng có thể đã tới trước khi hwnd được công bố.
        if control.is_stop_requested() {
            PostMessageW(hwnd, WM_APP_STOP, 0, 0);
        }
    }

    let mut msg: MSG = zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        DispatchMessageW(&msg);
    }

    // Lưới an toàn: dù vòng lặp thoát kiểu gì cũng không để chuột bị khoá
    // hay bàn phím bị chặn.
    ClipCursor(null());
    let state = STATE.with(|cell| cell.borrow_mut().take());
    match state {
        Some(state) => {
            if !state.hook.is_null() {
                UnhookWindowsHookEx(state.hook);
            }
            DeleteObject(state.font);
            DeleteObject(state.background);
            DeleteObject(state.border);
            state.reason.unwrap_or(StopReason::Cancelled)
        }
        None => StopReason::Cancelled,
    }
}

unsafe fn clip_cursor_to(hwnd: HWND) {
    let mut rect: RECT = zeroed();
    if GetClientRect(hwnd, &mut rect) == 0 {
        return;
    }
    let mut top_left = POINT { x: rect.left, y: rect.top };
    let mut bottom_right = POINT { x: rect.right, y: rect.bottom };
    ClientToScreen(hwnd, &mut top_left);
    ClientToScreen(hwnd, &mut bottom_right);
    let clip = RECT {
        left: top_left.x,
        top: top_left.y,
        right: bottom_right.x,
        bottom: bottom_right.y,
    };
    ClipCursor(&clip);
}

/// Ghi nhận lý do dừng (giữ lý do đầu tiên) rồi yêu cầu đóng ô.
unsafe fn request_close(hwnd: HWND, reason: StopReason) {
    with_state(|state| {
        if state.reason.is_none() {
            state.reason = Some(reason);
        }
    });
    PostMessageW(hwnd, WM_CLOSE, 0, 0);
}

unsafe fn on_timer(hwnd: HWND) {
    if let Some(reason) = with_state(|state| state.control.requested_reason()).flatten() {
        request_close(hwnd, reason);
        return;
    }
    if GetForegroundWindow() == hwnd {
        with_state(|state| state.unfocused_ticks = 0);
        // Windows tự gỡ clip trong nhiều tình huống nên phải áp lại định kỳ.
        clip_cursor_to(hwnd);
        return;
    }
    let give_up = with_state(|state| {
        state.unfocused_ticks += 1;
        state.activated || state.unfocused_ticks > FOCUS_GRACE_TICKS
    })
    .unwrap_or(true);
    if give_up {
        request_close(hwnd, StopReason::FocusLost);
    } else {
        SetForegroundWindow(hwnd);
    }
}

unsafe fn on_raw_input(lparam: LPARAM) {
    let mut raw: RAWINPUT = zeroed();
    let mut size = size_of::<RAWINPUT>() as u32;
    let copied = GetRawInputData(
        lparam as HRAWINPUT,
        RID_INPUT,
        (&mut raw as *mut RAWINPUT).cast::<c_void>(),
        &mut size,
        size_of::<RAWINPUTHEADER>() as u32,
    );
    if copied == u32::MAX || raw.header.dwType != RIM_TYPEMOUSE {
        return;
    }
    let mouse = raw.data.mouse;
    // Input do chính app target bơm (khi chạy chung máy): bỏ qua để không lặp vô hạn.
    if mouse.ulExtraInformation as usize == INJECT_TAG {
        return;
    }
    let buttons = mouse.Anonymous.Anonymous;
    with_state(|state| {
        state.on_mouse(
            u32::from(mouse.usFlags),
            u32::from(buttons.usButtonFlags),
            buttons.usButtonData,
            mouse.lLastX,
            mouse.lLastY,
        );
    });
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let resources = with_state(|state| (state.font, state.background, state.border, state.text.clone()));
    if let Some((font, background, border, text)) = resources {
        let mut client: RECT = zeroed();
        GetClientRect(hwnd, &mut client);
        let edge = 2;
        let inner = RECT {
            left: client.left + edge,
            top: client.top + edge,
            right: client.right - edge,
            bottom: client.bottom - edge,
        };
        FillRect(hdc, &client, border);
        FillRect(hdc, &inner, background);

        SetBkMode(hdc, BK_TRANSPARENT as _);
        SetTextColor(hdc, COLOR_TEXT);
        let previous_font = SelectObject(hdc, font);
        let margin = 10;
        let mut measured = RECT {
            left: inner.left + margin,
            top: 0,
            right: inner.right - margin,
            bottom: 0,
        };
        let format = DT_CENTER | DT_WORDBREAK | DT_NOPREFIX;
        DrawTextW(hdc, text.as_ptr(), text.len() as i32, &mut measured, format | DT_CALCRECT);
        let height = measured.bottom - measured.top;
        let mut area = RECT {
            left: inner.left + margin,
            top: ((client.bottom - height) / 2).max(inner.top),
            right: inner.right - margin,
            bottom: inner.bottom,
        };
        DrawTextW(hdc, text.as_ptr(), text.len() as i32, &mut area, format);
        SelectObject(hdc, previous_font);
    }
    EndPaint(hwnd, &ps);
}

/// Gỡ mọi thứ đang khoá input rồi huỷ cửa sổ.
unsafe fn teardown(hwnd: HWND) {
    ClipCursor(null());
    KillTimer(hwnd, TIMER_ID);
    let hook = with_state(|state| std::mem::replace(&mut state.hook, null_mut()));
    if let Some(hook) = hook.filter(|hook| !hook.is_null()) {
        UnhookWindowsHookEx(hook);
    }
    let mouse = RAWINPUTDEVICE {
        usUsagePage: HID_USAGE_PAGE_GENERIC,
        usUsage: HID_USAGE_GENERIC_MOUSE,
        dwFlags: RIDEV_REMOVE,
        hwndTarget: null_mut(),
    };
    RegisterRawInputDevices(&mouse, 1, size_of::<RAWINPUTDEVICE>() as u32);
    DestroyWindow(hwnd);
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_INPUT => {
            on_raw_input(lparam);
            // DefWindowProc dọn bộ đệm của WM_INPUT.
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_TIMER => {
            on_timer(hwnd);
            0
        }
        WM_ACTIVATE => {
            if wparam & 0xFFFF != 0 {
                with_state(|state| state.activated = true);
                clip_cursor_to(hwnd);
            } else if with_state(|state| state.activated).unwrap_or(false) {
                request_close(hwnd, StopReason::FocusLost);
            }
            0
        }
        WM_APP_STOP => {
            let reason = with_state(|state| state.control.requested_reason())
                .flatten()
                .unwrap_or(StopReason::Cancelled);
            request_close(hwnd, reason);
            0
        }
        WM_CLOSE => {
            teardown(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION {
        let key = &*(lparam as *const KBDLLHOOKSTRUCT);
        // Phím do phần mềm bơm (bộ gõ, chính app target) không chuyển tiếp.
        if key.flags & LLKHF_INJECTED == 0 {
            let outcome = with_state(|state| {
                let outcome = state.on_key(
                    key.vkCode as u16,
                    key.scanCode as u16,
                    key.flags & LLKHF_EXTENDED != 0,
                    key.flags & LLKHF_UP == 0,
                );
                (outcome, state.control.hwnd.load(Ordering::SeqCst))
            });
            match outcome {
                Some((KeyOutcome::Stop, hwnd)) => {
                    PostMessageW(hwnd as HWND, WM_CLOSE, 0, 0);
                    return 1;
                }
                Some((KeyOutcome::Swallow, _)) => return 1,
                None => {}
            }
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}
