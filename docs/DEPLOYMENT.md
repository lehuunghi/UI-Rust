# Triển khai và vận hành

## Cấu hình tối thiểu

- `DATABASE_URL`: PostgreSQL; dùng tài khoản riêng cho panel.
- `APP_KEY`: 32 byte dạng 64 ký tự hex; dùng `openssl rand -hex 32` hoặc `ui-rust keygen`.
- `APP_URL`: origin mà trình duyệt truy cập, ví dụ `https://panel.example.com`.
- `BIND`: mặc định `127.0.0.1:8080`; container dùng `0.0.0.0:8080`.
- Chạy `serve` và `worker` thành hai tiến trình được giám sát. `/healthz` kiểm tra tiến trình;
  `/readyz` kiểm tra kết nối PostgreSQL. Worker xử lý SMTP, sync và vòng đời dịch vụ.
- Migrations chạy khi khởi động. Account phục vụ migration cần DDL; có thể chạy `migrate`
  bằng account triển khai trước khi dùng account runtime bị giới hạn theo chính sách riêng.

APP_KEY phải giống nhau ở web, worker và công cụ backup. Mất khóa sẽ không giải mã được
API token, tác vụ chứa mật khẩu, TOTP hoặc backup. Sao lưu khóa riêng với database.

## Reverse proxy

Ví dụ Nginx phía trước ứng dụng:

```nginx
server {
    listen 443 ssl;
    server_name panel.example.com;
    # ssl_certificate và ssl_certificate_key: cấu hình certificate của bạn
    client_max_body_size 64k;
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

Đặt APP_URL theo URL HTTPS bên ngoài để cookie có Secure. Không đưa panel trực tiếp ra
Internet bằng HTTP. Mutation từ trình duyệt kiểm tra Origin và CSRF; không bật CORS `*`.
Ứng dụng không tin X-Forwarded-For để tránh giả IP. Thêm rate limit theo IP ở proxy nếu cần.

## Khởi tạo admin

Không có seed mật khẩu. Đặt ADMIN_EMAIL/ADMIN_PASSWORD trong môi trường và chạy
`ui-rust create-admin` hoặc `docker compose run --rm -e ADMIN_EMAIL -e ADMIN_PASSWORD web create-admin`.
Sau đó xóa hai biến, đăng nhập và bật TOTP. Tạo gói, server và khách hàng từ UI.
Mật khẩu tối thiểu 12 byte. Tham số mật khẩu không được đặt trong URL hay ghi log.

## Stalwart

Cấu hình trong **Server Stalwart**, chọn primary và dry-run/live. Token có thể là Bearer,
`user:password`, hoặc `basic:<base64>`, tương thích cách chọn Authorization của nguồn PHP.
Dùng HTTPS với chứng chỉ hợp lệ cho kết nối live. Không theo redirect gửi credential.

Worker chỉ xử lý tác vụ trên server đã gán. Version cấu hình thay đổi sẽ làm tác vụ chờ
chuyển sang failed để quản trị kiểm tra. Lệnh có kết quả không rõ ràng không tự chạy lại:
tra cứu Stalwart trước, tránh tạo trùng tài nguyên. Với thay đổi nâng cao, kế hoạch
preview có hiệu lực 15 phút, chỉ người tạo được execute một lần; `executing` kéo dài sau
sự cố hoặc `unconfirmed` đều yêu cầu đối chiếu trạng thái remote.

Các bản ghi dry-run không được chuyển trực tiếp sang live. Tạo database thử riêng.
Tính năng JMAP cần được thử với phiên bản/schema Stalwart đang triển khai; CI dùng dry-run,
không kết nối server mail thật.

## SePay

- Đặt SEPAY_ACCOUNT_NUMBER và SEPAY_BANK_CODE.
- Đặt SEPAY_API_KEY cho `Authorization: Apikey ...`, hoặc SEPAY_HMAC_SECRET.
- Nếu cả hai được đặt, chỉ chấp nhận HMAC: `X-SePay-Timestamp` và
  `X-SePay-Signature: sha256=<HMAC_SHA256(timestamp + '.' + raw_body)>`.
- URL webhook: `<APP_URL>/sepay/webhook`, POST JSON. GET chỉ xác nhận endpoint tồn tại.
- ID bằng 0 là kiểm tra kết nối, không ghi ledger. Sai tài khoản nhận bị từ chối.
- Trạng thái unmatched/late/additional cần người quản trị xem xét. Unmatched có thể gán
  hóa đơn pending với lý do; trạng thái late không tự kích hoạt dịch vụ.

CI kiểm tra replay và thanh toán đồng thời nhưng không thay thế thử nghiệm SePay thật
trên tài khoản ngân hàng và cấu hình webhook của bạn.

## SMTP / 2FA

SMTP_HOST, SMTP_USER, SMTP_PASSWORD, SMTP_FROM dùng STARTTLS port 587. Worker phải chạy
để gửi OTP và email đặt lại mật khẩu. OTP 5 phút, reset link 30 phút. TOTP dùng bước 30 giây;
đồng bộ thời gian máy chủ. Admin đầu tiên không bật 2FA tự động để tránh khóa hệ thống
trước khi có SMTP; phải bật TOTP sau khi tạo.

## Backup và restore

`ui-rust backup` tạo `backups/ui-<UTC>-<random>.pgenc`. Native host cần pg_dump/pg_restore
phiên bản tương thích PostgreSQL; Docker image có client 17. Backup dùng pg_dump custom
format, không lưu role/ACL hệ thống; AES-256-GCM kiểm tra tính toàn vẹn trước restore.

```sh
docker compose run --rm web backup
# Files stay in the panel_backups named volume.
docker compose cp web:/app/backups ./backup-export
```

Chép `.pgenc` ra lưu trữ tách biệt, ví dụ bằng `rclone`, và sao lưu APP_KEY riêng.
Đặt lịch bằng cron hoặc Windows Task Scheduler. Chưa có retention/offsite UI tích hợp.
Backup hiện được mã hóa trong RAM; dùng native snapshot/backup pipeline khác cho database
quá lớn so với RAM khả dụng.

Restore chỉ trên database đã chuẩn bị và đã dừng web/worker. Thử trên một database riêng
trước; lệnh có `--clean` và sẽ thay thế các object của bản backup trong transaction:

```sh
# Set DATABASE_URL to the isolated restore database and APP_KEY to the backup's key.
ui-rust restore backups/example.pgenc --confirm-db ui_restore
```

Tên sau --confirm-db phải khớp DATABASE_URL. Tạo database/role trước restore. Kiểm tra
schema, row counts, ledger, quota và mapping Stalwart sau restore. Backup này chỉ chứa
PostgreSQL của panel; mailbox/blob/config/vault của Stalwart phải được backup riêng.

## Cập nhật

Build image từ commit đã kiểm thử, backup database, triển khai image mới và theo dõi
readyz/worker. Không dùng trình cập nhật PHP để cập nhật binary Rust. Với migration mới,
không rollback binary tùy tiện; xác minh tương thích schema hoặc restore bản sao đầy đủ.
