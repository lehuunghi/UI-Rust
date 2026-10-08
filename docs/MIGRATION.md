# Phân tích và đối chiếu với bản PHP

Nguồn: `lehuunghi/UI`, commit `6c3cb21de76c77c2af5739b463cd69fa1019207f`.
Phân tích `public/index.php`, toàn bộ controller/service/view và các file SQL trong
snapshot đó. Repo nguồn giữ nguyên; repo này là một ứng dụng độc lập.

## Kiến trúc nguồn và bản Rust

| Thành phần | PHP/MySQL | Rust/PostgreSQL |
|---|---|---|
| HTTP | Router PHP, controller, PHP views | Axum, JSON API, HTML/CSS/JS cùng origin |
| CSDL | PDO MySQL, ENUM, AUTO_INCREMENT | SQLx PostgreSQL, CHECK, BIGSERIAL, JSONB |
| Session | PHP session + panel_sessions | Cookie opaque, SHA-256 token lưu trong PostgreSQL |
| Mật khẩu | PHP password_hash | Argon2id; không tạo tài khoản mặc định |
| Bí mật | SecretBox PHP | AES-256-GCM với khóa APP_KEY riêng |
| Giá | PHP float | rust_decimal, NUMERIC(12,2), API serialize thành chuỗi |
| Tác vụ nền | Nhiều script PHP/Task Scheduler | `ui-rust worker`, giao dịch và khóa PostgreSQL |
| Giao diện | Template PHP | Giao diện mới; giữ các đường dẫn chính tiếng Việt |
| Update | Tự thay thế mã PHP trong panel | Build image/binary mới rồi triển khai; không cho web tự thay mã thực thi |

## Nghiệp vụ đã chuyển

| Nghiệp vụ | Trạng thái và khác biệt |
|---|---|
| Đăng ký, đăng nhập, session | Có; CSRF và kiểm tra Origin; rate limit tài khoản trong PostgreSQL |
| 2FA | TOTP và email OTP; người quản trị đầu tiên phải bật TOTP sau khi tạo |
| Quên mật khẩu | Token băm, một lần, 30 phút; SMTP STARTTLS và worker bắt buộc |
| Đổi mật khẩu, thu hồi phiên | Có; đổi mật khẩu vô hiệu hóa toàn bộ phiên |
| Quản trị viên | Super admin và manager theo danh sách quyền; không có impersonation |
| Admin phụ | Phân quyền và phạm vi domain/nhóm; backend kiểm tra từng thao tác |
| Gói dịch vụ | Thêm/sửa, min/max hạn mức, giá khuyến mãi, thời hạn và dung lượng |
| Định giá | Đúng công thức `email_qty × effective_monthly_price × billing_months + extra_domains × extra_domain_price`; domain bổ sung không nhân số tháng |
| Đơn hàng/gia hạn | Tính lại giá trên server; gia hạn bắt đầu sau ngày cuối gói cha; không kích hoạt lại invoice đã có marker |
| SePay webhook | API key hoặc HMAC timestamp ±300 giây; số tiền nguyên; tài khoản nhận khớp cấu hình; cộng dồn, chống trùng ID/reference, xử lý đồng thời |
| Đối soát thủ công | Gán giao dịch unmatched cho invoice pending, kèm lý do và nhật ký |
| SePay User API v2 | Chưa chuyển trình kéo dữ liệu API định kỳ/sandbox; webhook và đối soát thủ công đã có |
| Dùng thử | Khách hàng đã đăng nhập gửi yêu cầu; admin duyệt 14 ngày; chưa nhận/tải hồ sơ định danh đính kèm |
| Domain | Quản lý, đồng bộ, tạm ngưng/xóa, TXT xác minh quyền sở hữu; DNS zone được giữ từ phản hồi Stalwart nếu có |
| DNS | Truy vấn TXT qua Cloudflare DNS-over-HTTPS; chưa kiểm tra đầy đủ MX/SPF/DKIM/DMARC/STARTTLS như module PHP |
| Hộp thư | Tạo/xóa, đổi tên hiển thị, đổi mật khẩu, tạm ngưng/bật lại với mật khẩu mới |
| Nhóm và alias | Có; kiểm tra sở hữu, server tương ứng, địa chỉ trùng và quyền của gói |
| Hạn mức | Khóa dòng khách hàng trước khi đếm/tạo, tránh vượt hạn mức do request đồng thời |
| Vòng đời | Hết hạn gói; worker tạm ngưng tài nguyên khi khách bị khóa hoặc quá hạn 15 ngày. Domain được bật lại sau gia hạn; hộp thư cần mật khẩu mới |
| Nhiều server | Gán server cố định cho tài nguyên; không fallback sang server khác khi lỗi; tác vụ kiểm tra version cấu hình |
| Stalwart JMAP | Kiểm tra call ID, kết quả từng create/update/destroy, lỗi object; giới hạn response 4 MB và timeout |
| Vận hành nâng cao | Công cụ super-admin truy vấn và preview/execute các method trong allowlist từ nguồn, gồm queue, task, role/tenant, DKIM, API/app credentials, throttles và báo cáo |
| Luồng vận hành chuyên biệt | Chưa tái tạo từng wizard PHP cho DKIM khẩn cấp, mailbox migration, tenant binding, báo cáo tháng, telemetry/SIEM, TLS và recovery plan |
| Backup PostgreSQL | CLI Rust gọi pg_dump, mã hóa AES-GCM; restore cần chỉ rõ database đích. Schedule/offsite qua cron/Task Scheduler/rclone |
| Backup mailbox/blob/config Stalwart | Không nằm trong backup PostgreSQL; các workflow backup/recovery đa cloud của PHP chưa được chuyển |
| Trang chủ và nội dung | Giá lấy từ database, chỉnh nội dung theo section/language, cấu hình public, escape trên trình duyệt |
| Ngôn ngữ | Giao diện chính VI/EN và bảng dịch riêng; chưa chuyển toàn bộ catalog ~200 KB mỗi ngôn ngữ và language editor của PHP |
| SMTP | Email OTP/reset; chưa chuyển toàn bộ mẫu email, chiến dịch gửi hàng loạt và thông báo gia hạn của PHP |
| CSV/import tài khoản | Chưa có UI import/export hàng loạt của bản PHP |

Các màn hình trên chỉ hiển thị dữ liệu thật hoặc dữ liệu dry-run được đánh dấu rõ.
Không có dữ liệu mẫu khách hàng, mật khẩu mặc định hay token dịch vụ trong repo.
Các bảng giữ lại từ schema nguồn phục vụ đối chiếu; bảng chưa có service Rust không
được coi là một tính năng đã triển khai.

## Chuyển dữ liệu hiện hữu

Đây là **schema migration cho cài mới**, không phải data migration tự động từ MySQL.
Không dùng pgloader hoặc import dump mà chưa lập mapping và thử trên bản sao:

1. Xuất dữ liệu nghiệp vụ có chọn lọc vào môi trường riêng; không commit dump.
2. Chuẩn hóa email/domain về chữ thường, kiểm tra các trùng lặp trước khi tạo unique index.
3. Giữ quan hệ customer/package/subscription/domain/account và mapping server; reset sequence sau khi giữ ID cũ.
4. Chuyển DATETIME theo timezone nguồn thành TIMESTAMPTZ; xác minh ngày tính cước.
5. Các ciphertext của PHP **không tương thích** AES-GCM Rust. Nạp lại API token qua UI, buộc đặt lại TOTP và mật khẩu phù hợp. Không copy session hay reset token.
6. Đối chiếu ledger, paid_at, activated_at, bank reference, invoice_code để tránh thanh toán lặp.
7. Chạy dry-run và kiểm thử tenant, quota, renewal, webhook; kiểm tra đối chiếu thủ công trước cutover.

Không đưa các bản ghi `dry-*` vào server live. Dùng database thử nghiệm riêng cho dry-run.
Ứng dụng mới dùng ngày UTC cho kỳ dịch vụ; cần thống nhất quy tắc ngày trước khi chuyển
hệ thống PHP đang tính ngày theo timezone khác.
