# SmartMaupad

Điều khiển chuột và bàn phím của một máy Windows khác qua mạng (LAN hoặc Tailscale). Viết bằng Rust, gồm hai file `.exe`:

- **`smartmaupad-target.exe`** — chạy trên máy *bị* điều khiển. Mở một cổng TCP, nhận sự kiện và bơm vào Windows.
- **`smartmaupad-control.exe`** — chạy trên máy *điều khiển*. Nhập IP, cổng, mật khẩu của target rồi bấm **Bắt đầu**.

## Build

```bash
cargo build --release
```

Kết quả nằm ở `target\release\smartmaupad-target.exe` và `target\release\smartmaupad-control.exe`. Hai file chạy độc lập, không cần cài thêm gì; chép sang máy khác là dùng được.

## Cách dùng

### Trên máy target

1. Mở `smartmaupad-target.exe`.
2. Đặt **Cổng** (mặc định `45800`) và **Mật khẩu**, bấm **Bật nghe**. Lần đầu Windows Firewall sẽ hỏi, chọn cho phép.
3. Ghi lại IP ở mục **IP của máy này**. Dùng Tailscale thì lấy dòng ghi chú `Tailscale` (dạng `100.x.y.z`).

Tuỳ chọn:

- **Chỉ ghi log, không bơm input** — hiện sự kiện nhận được trong Nhật ký thay vì điều khiển máy. Dùng để thử.
- **Tự bật nghe khi mở app** — mở app là nghe luôn bằng cổng và mật khẩu đã lưu.

### Trên máy control

1. Mở `smartmaupad-control.exe`.
2. Nhập **IP target**, **Cổng**, **Mật khẩu** giống bên target.
3. Bấm **Bắt đầu**. Sau khi kết nối và xác thực xong, một ô 200x200px hiện ra giữa màn hình và con trỏ bị khoá trong ô đó. Từ lúc này:
   - di chuyển, click, kéo-thả, cuộn chuột đều chạy trên target;
   - mọi phím gõ (kể cả phím Windows, Alt+Tab, Alt+F4) đều sang target.
4. **Để dừng, nhấn phím tắt dừng trên bàn phím — mặc định `Ctrl+Alt+F12`.** Vì chuột bị khoá trong ô, đây là cách duy nhất để dừng bằng tay. Phím tắt này không bị gửi sang target.

Đổi phím tắt: bấm vào nút ở dòng **Phím tắt dừng** rồi nhấn tổ hợp mới (Ctrl/Alt/Shift + chữ, số hoặc F1–F24).

**Độ nhạy chuột** nhân tốc độ con trỏ trên target; giảm xuống nếu con trỏ chạy quá nhanh.

Phiên cũng tự dừng và nhả chuột khi mất kết nối quá 5 giây hoặc khi ô điều khiển mất focus.

**Lối thoát khẩn cấp:** nếu quên phím tắt, bấm `Ctrl+Alt+Del` rồi `Esc`. Windows không cho app nào chặn tổ hợp này; nó làm ô điều khiển mất focus nên phiên dừng ngay.

## Dùng qua Tailscale

Cài Tailscale và đăng nhập cùng một tailnet trên cả hai máy, rồi nhập IP `100.x.y.z` của target vào app control. Không cần mở cổng trên router.

## Bảo mật

- Control phải biết mật khẩu mới điều khiển được. Xác thực kiểu challenge-response (HMAC-SHA256) nên mật khẩu không đi qua mạng; sai mật khẩu bị từ chối sau 1 giây.
- Dữ liệu sau khi xác thực **không được app mã hoá**. Qua Tailscale thì WireGuard đã mã hoá toàn bộ. Trên LAN thuần, người nghe lén được đường truyền có thể đọc phím bạn gõ, nên đừng dùng trên mạng không tin cậy nếu không có Tailscale.
- Target nghe trên mọi card mạng (`0.0.0.0`). Đừng chuyển tiếp cổng này ra Internet.
- Mật khẩu lưu trong `%APPDATA%\SmartMaupad\*.toml`, mã hoá bằng DPAPI theo tài khoản Windows.

## Giới hạn

- Target chạy quyền thường không điều khiển được cửa sổ chạy quyền admin (Task Manager, app "Run as administrator"). Muốn điều khiển chúng thì chạy `smartmaupad-target.exe` bằng **Run as administrator**.
- Không điều khiển được hộp thoại UAC và màn hình khoá; `Ctrl+Alt+Del` không chuyển tiếp được.
- Con trỏ trên target còn chịu thêm gia tốc chuột của chính máy đó ("Enhance pointer precision").
- Nếu máy control đang bật bộ gõ tiếng Việt (Unikey, EVKey…), hãy chuyển về chế độ gõ tiếng Anh khi điều khiển; việc gõ dấu do bộ gõ trên target đảm nhận.
- Giao diện vẽ bằng OpenGL, cần driver đồ hoạ hỗ trợ OpenGL 2.0 trở lên.
- Chỉ thử chạy cả hai app trên cùng một máy khi target bật **Chỉ ghi log**.

## Cấu trúc mã nguồn

| Thư mục | Nội dung |
|---|---|
| `crates/shared` | Giao thức nhị phân, bắt tay xác thực, lưu cấu hình |
| `crates/target` | Server TCP, bơm input bằng `SendInput`, giao diện |
| `crates/control` | Ô bắt chuột (Raw Input, keyboard hook, `ClipCursor`), kết nối, giao diện |

## Kiểm thử

```bash
cargo test --workspace
```
