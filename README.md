# UI-Rust — Email Business

Rust/Axum + PostgreSQL rewrite of `lehuunghi/UI`, analyzed at commit
`6c3cb21de76c77c2af5739b463cd69fa1019207f`. The web server and background worker are
Rust; the browser interface uses HTML/CSS/JavaScript. No PHP or MySQL is required.

**This is a new implementation, not a drop-in replacement of every PHP module.**
See [the migration matrix](docs/MIGRATION.md) for implemented features, differences,
and remaining work. Do not point it at a production database from the PHP application.

## Hướng dẫn cài đặt A–Z

Xem [cài đặt Docker/VPS và vận hành](docs/INSTALL_VI.md),
[cài trực tiếp bằng systemd](docs/INSTALL_NATIVE_VI.md) và
[cấu hình Stalwart, SMTP, SePay, backup/recovery](docs/CONFIG_SERVICES_VI.md).
Lấy nguồn từ nhánh `main`, ghi lại SHA và triển khai commit đã kiểm thử:

```sh
git clone --branch main --single-branch https://github.com/lehuunghi/UI-Rust.git
cd UI-Rust
git rev-parse HEAD
```

Có thể dùng gói nguồn kèm `SHA256SUMS` từ nguồn tin cậy để cài offline.
Giữ SHA/checksum của từng release để dựng lại hoặc chuẩn bị rollback.

## Có gì trong bản này?

- Trang chủ, bảng giá, giao diện quản trị và khách hàng, các đường dẫn tiếng Việt.
- Phiên đăng nhập PostgreSQL, Argon2id, CSRF, giới hạn đăng nhập, email OTP/TOTP,
  khôi phục mật khẩu, thu hồi phiên và mã hóa bí mật bằng AES-256-GCM.
- Khách hàng, quản trị viên có quyền hạn, admin phụ với phạm vi domain/nhóm.
- Gói dịch vụ, đặt gói, gia hạn, hóa đơn, dùng thử 14 ngày có duyệt.
- Giá được tính lại trên server theo công thức của PHP; dùng số thập phân chính xác.
- SePay API-key/HMAC webhook, chống trùng giao dịch, cộng dồn thanh toán,
  khóa PostgreSQL khi kích hoạt và lưu giao dịch không khớp để kiểm tra.
- Domain, DNS TXT verification, hộp thư, nhóm, alias, đồng bộ Stalwart, nhiều server và dry-run.
- Backup/restore PostgreSQL mã hóa bằng CLI Rust; worker xử lý tạm ngưng sau hết hạn.
- Công cụ JMAP quản trị: tra cứu, xem trước thay đổi, xác nhận một lần, kiểm tra
  version server và phản hồi JMAP. Lệnh không được xác nhận không tự chạy lại.
- Nội dung trang chủ, catalog VI/EN và trình sửa ngôn ngữ, 11 mẫu email và thông báo gia hạn.
- Import/export CSV, hồ sơ trial mã hóa, impersonation, SePay API live/sandbox.
- Monitoring, báo cáo tháng, tenant binding, phân bổ server và migration preview.
- Backup mailbox/recovery qua worker riêng, DKIM khẩn cấp và vault mã hóa.
- Xem [đối chiếu chức năng](docs/PARITY.md) để biết bằng chứng và phần còn thiếu.
- 52 bảng được chuyển từ schema gốc sang PostgreSQL, cộng các bảng runtime Rust.
  Có schema không đồng nghĩa đã chuyển toàn bộ logic của module tương ứng.

## Chạy bằng Docker Compose

1. Sao chép `.env.example` thành `.env`.
2. Tạo khóa: `openssl rand -hex 32`, đặt vào `APP_KEY`.
3. Thêm `POSTGRES_PASSWORD=<mật khẩu mạnh>` vào `.env`. Dùng mật khẩu không chứa
   ký tự dành riêng của URL, hoặc cấu hình riêng `DATABASE_URL` đã percent-encode.
4. Đặt `APP_URL` đúng origin bên ngoài, không có dấu `/` cuối. Dùng HTTPS khi triển khai.
5. Chạy:

```sh
docker compose up -d --build
```

Tạo admin một lần bằng service `web`:

```sh
# Export ADMIN_PASSWORD in your shell first; avoid putting it in shell history.
docker compose run --rm -e ADMIN_EMAIL=admin@example.com -e ADMIN_PASSWORD web create-admin
```

Mở `http://localhost:8080/dang-nhap`. Đăng nhập, bật TOTP tại **Tài khoản**, tạo gói
và cấu hình **Server Stalwart**. Bắt đầu với `dry_run=1`, kiểm tra rồi mới dùng live.
Không có tài khoản hoặc mật khẩu quản trị cài sẵn.

## Chạy từ mã nguồn

Cần Rust stable và PostgreSQL 17 (hoặc bản PostgreSQL tương thích).

```sh
cp .env.example .env
# Điền DATABASE_URL, APP_KEY và APP_URL
cargo run -- migrate
cargo run -- create-admin
cargo run -- serve
# Terminal riêng:
cargo run -- worker
```

Migrations chạy khi khởi động, có khóa của SQLx. `keygen` chạy không cần database.
`serve` không chạy worker trong cùng tiến trình.

## Kiểm thử

```sh
cargo fmt --all --check
DATABASE_URL=postgres://postgres:postgres@localhost/ui_test cargo test --all-targets
node --check static/app.js
```

Integration tests cần tài khoản PostgreSQL có quyền tạo database kiểm thử riêng.
GitHub Actions chạy PostgreSQL service và kiểm thử HTTP qua Axum, CSRF, cô lập khách
hàng, hạn mức, thanh toán từng phần/trùng đồng thời, dry-run và thu hồi phiên.
Không dùng database thật để chạy test.

Chi tiết: [Migration](docs/MIGRATION.md) · [Deployment](docs/DEPLOYMENT.md).
