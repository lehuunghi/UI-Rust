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

## Nghiệp vụ và mức độ tương đương

Xem [bảng đối chiếu và bằng chứng kiểm thử](PARITY.md). Bảng này phân biệt
implementation đã có, kiểm chứng trên fixture, yêu cầu kiểm chứng dịch vụ thật
và những luồng PHP còn chưa chuyển. Hiện chưa thể coi hai bản tương đương 100%.

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
