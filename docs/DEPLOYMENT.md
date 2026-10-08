# Triển khai và vận hành

Hướng dẫn từng bước: [cài đặt A–Z](INSTALL_VI.md), [native Linux](INSTALL_NATIVE_VI.md),
[cấu hình dịch vụ](CONFIG_SERVICES_VI.md).

Lấy nguồn từ nhánh `main` của [lehuunghi/UI-Rust](https://github.com/lehuunghi/UI-Rust),
ghi lại SHA thực tế bằng `git rev-parse HEAD` và triển khai commit đã kiểm thử.
Có thể dùng source archive kèm checksum từ nguồn tin cậy để cài offline. Giữ
commit/checksum, cấu hình và backup của mỗi release; nhánh `main` có thể thay đổi.

## Cấu hình tối thiểu

- `DATABASE_URL`: PostgreSQL; dùng tài khoản riêng cho panel.
- `APP_KEY`: 32 byte dạng 64 ký tự hex; dùng `openssl rand -hex 32` hoặc `ui-rust keygen`.
- `APP_URL`: origin mà trình duyệt truy cập, ví dụ `https://panel.example.com`.
- `BIND`: mặc định `127.0.0.1:8080`; container dùng `0.0.0.0:8080`.
- Chạy `serve` và `worker` thành hai tiến trình được giám sát; thêm `mailbox-worker` và `recovery-worker` khi dùng backup mail. `/healthz` kiểm tra tiến trình;
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
    client_max_body_size 16m;
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
Có UI lịch/retention/offsite panel, worker xử lý lịch và retry upload. Đích là remote rclone đã cấu hình trên máy; mount cùng thư mục backup và cùng cấu hình rclone cho web/worker.
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


## Backup mailbox và recovery server

Hai worker riêng xử lý tác vụ dài: `ui-rust mailbox-worker` và
`ui-rust recovery-worker`; dùng `mailbox-once`/`recovery-once` để kiểm tra một vòng.
Docker Compose có profile `mail-recovery`, nhưng phải bổ sung binary/mount cấu hình
thực trước khi bật. Image mặc định chứa OpenSSL, rclone và PostgreSQL client;
Vandelay và Stalwart CLI cần được người vận hành cài từ nguồn chính thức với
checksum/signature được kiểm chứng, phù hợp phiên bản server.

- `MAILBOX_VANDELAY_BIN`: đường dẫn executable Vandelay, mặc định `vandelay`.
- `RECOVERY_STALWART_CLI_BIN`: đường dẫn executable, mặc định `stalwart-cli`.
- `RECOVERY_SOURCES_FILE`: JSON operator-owned, nằm ngoài public, chứa `profiles`.
  Mỗi nguồn khai báo `server_origin`, `snapshot_marker`, `components` (tối thiểu
  data/blob/bootstrap), `required_components`, `max_snapshot_age_seconds`.
  Config snapshot có thể dùng `config_plan_path` NDJSON hoặc CLI snapshot.
- Marker phải có snapshot_id, server_origin, created_at và consistent=true;
  config_plan_path cần config_sha256 trong marker. Không trỏ vào database đang ghi rồi tự đặt
  consistent=true: tạo snapshot/checkpoint nhất quán bằng công cụ backend thực.
- Đích recovery khai báo restore_root tuyệt đối, isolated_rehearsal=true,
  outbound_isolated=true; adapter operator-owned thực hiện nạp backend và trả
  receipt theo contract trong `src/recovery_sources.rs`. Không đưa server thật
  đang phục vụ mail làm đích diễn tập.

Dùng cùng APP_KEY và mount `/app/backups` giữa web và mọi worker. Tệp nguồn
snapshot, rclone config và adapter phải được mount riêng với quyền tối thiểu.
Không mount secret dưới `static`. Không thay các executable bằng fixture test
khi vận hành thực. Chạy preview và kiểm tra mail/thư mục/attachment/queue trên
môi trường cô lập trước khi ghi biên bản thành công.

Archive mailbox/recovery có giới hạn 256 MiB; xác minh dung lượng và RAM trước
khi chọn pipeline này. Cấu hình lifecycle storage cho retention cloud của chúng.
Xem [các giới hạn và luồng chưa tương đương](PARITY.md).


## Trial công khai và reconciliation

`/trial` cho phép khách chưa đăng nhập gửi hồ sơ; có Origin check, giới hạn theo
email và giới hạn 200 hồ sơ/ngày toàn panel. Đặt rate limit theo IP ở reverse proxy
cho endpoint public. Duyệt khách mới yêu cầu SMTP và mẫu password_reset bật;
không tạo mật khẩu mặc định, không trả mật khẩu qua UI. Liên kết đặt mật khẩu
hết hạn 30 phút, có thể yêu cầu lại tại quên mật khẩu nếu email gửi chậm.

Reconciliation chỉ super admin và người tạo bản preview được apply trong 30 phút.
Chọn tối đa 50 mục/lần; chỉ sửa resource đã liên kết và đủ điều kiện, không nhận
quyền quản lý hoặc xóa orphan. `uncertain` yêu cầu đọc remote và tạo preview mới,
không tự chạy lại. Alias, quota và mapping thay đổi sẽ làm preview cũ stale.

Backup panel có tác vụ retrieve qua worker, chỉ từ cloud path đã ghi và còn khớp
policy hiện tại. Tệp được kiểm tra kích thước/checksum/AES-GCM trước khi đăng ký
cục bộ, không ghi đè tệp hiện hữu. Bản vừa retrieve được giữ ít nhất 24 giờ.
`BACKUP_RCLONE_BIN` có thể trỏ executable rclone operator-owned nếu cần.
