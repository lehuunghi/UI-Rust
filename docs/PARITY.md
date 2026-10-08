# Đối chiếu chức năng với lehuunghi/UI

Đối chiếu ngày 2026-10-08 với mã PHP tại commit
`6c3cb21de76c77c2af5739b463cd69fa1019207f`. Đây là snapshot cụ thể,
không phải cam kết tương đương với mọi thay đổi tương lai trên GitHub.

**Bản Rust chưa tương đương 100% với bản PHP.** Các nhóm chức năng bên dưới
đã có implementation; kiểm thử mock không chứng minh khả năng tương thích với
một server Stalwart hoặc tài khoản SePay thật.

## Các nhóm đã triển khai

| Nhóm | Bản Rust và bằng chứng |
|---|---|
| Auth và quyền | Session PostgreSQL, Argon2id, CSRF/Origin, TOTP/email OTP, reset mật khẩu, thu hồi phiên, admin permission, phạm vi admin phụ; test TOTP/replay/session/quota |
| Hỗ trợ khách hàng | Impersonation gắn với phiên admin, xoay CSRF, kiểm tra thu hồi quyền và mật khẩu admin; test revocation |
| Billing | Gói/đơn hàng/gia hạn, Decimal, hạn mức và vòng đời; test công thức PHP, đồng thời và kích hoạt một lần |
| SePay | Webhook, đối soát thủ công, API v2 live/sandbox, kéo định kỳ opt-in, chống trùng API/webhook; test bằng HTTP fixture, sandbox không ghi ledger |
| Trial | Yêu cầu 14 ngày công khai hoặc đã đăng nhập; duyệt khách mới tạo tài khoản và queue liên kết đặt mật khẩu, hồ sơ JPG/PNG/PDF mã hóa, admin tải có audit, retention hồ sơ đã xử lý; test quyền và mã hóa |
| Mail và nhiều server | Domain/hộp thư/nhóm/alias, TXT ownership, sync job, gán server cố định; import CSV/TXT tối đa 200 dòng và CSV export; test quota, scope và bí mật |
| Phân bổ | Server mặc định theo khách cho domain mới, giới hạn tài khoản theo server có khóa đồng thời, dung lượng quan sát và chi phí cấu hình; test hai khách tạo đồng thời |
| DNS | MX/SPF/DKIM/DMARC/PTR và STARTTLS, kết quả chưa biết tách khỏi lỗi; test parser và policy, chưa kiểm chứng mail domain thật |
| Quản trị Stalwart | 21 loại collection, action có tên, queue/task/role/tenant/credentials/throttle/IP/certificate/DKIM; preview, local/remote guard, quyền và version mới trước execute; test stale/single-use/secret |
| Credential | Kết quả bí mật mã hóa, chỉ người tạo với mật khẩu admin được xem một lần trong 15 phút; test quyền và một lần |
| Reconciliation | Preview domain/account/quota/alias theo mapping; local hash, remote projection và ifInState trước ghi, xác minh sau ghi; uncertain không tự retry, không xóa/nhận orphan; test stale/replay/post-write |
| Migration domain | Preview mapping/inventory/quota/version, checklist backup/restore/DNS/delivery, xác nhận một lần; chỉ chuyển liên kết sau khi người vận hành đã chuyển dữ liệu mail; test stale và alias |
| Tenant | Binding khách-server sau khi xác minh tenant và membership domain; tác vụ domain kế thừa binding |
| Báo cáo | DMARC XML, TLS JSON, ARF JSON, DSN, gzip giới hạn giải nén; history tháng UTC và CSV/NDJSON theo phạm vi domain; test DTD, compression, dedupe, quyền |
| Monitoring | Lịch opt-in, API/queue/job/worker/backup, dung lượng tài khoản, certificate expiry, cảnh báo và email; unknown/dry-run tách riêng; test quan sát bằng server fixture |
| Email | 11 template, gửi thông báo theo hàng đợi mã hóa, broadcast và nhắc gia hạn idempotent; test queue, không gửi SMTP thật |
| Nội dung/ngôn ngữ | CMS trang chủ, section/item/reorder/SEO, catalog VI/EN và language editor; test cập nhật không ghi đè section khác |
| Backup panel | PG dump mã hóa, lịch, retention, rclone upload/retry/retrieve có kiểm tra checksum/mã hóa và grace 24 giờ, download và restore DB rỗng với xác nhận; test pg_dump/pg_restore PostgreSQL 17 thật |
| Backup mailbox | Vandelay qua worker riêng, SQLite quick_check, archive mã hóa/checksum, cloud upload/retrieve, restore bổ sung vào hộp thư thử nghiệm sau preview một lần; test SQLite thật và CLI fixture |
| Recovery Stalwart | Snapshot marker/inventory/hash, data/blob/bootstrap và cấu hình/vault, CLI dry-run, diễn tập server cô lập, adapter receipt và biên bản RPO/RTO; test tệp thật và CLI/API fixture |
| DKIM khẩn cấp | RSA 2048 bằng OpenSSL, stage vào vault mã hóa, xác minh DNS rồi activate/retire/revoke, trạng thái uncertain khi lỗi; test RSA thật và native API fixture |

## Các khác biệt còn tồn tại

Đây là công việc sản phẩm còn lại, không phải chỉ thiếu credential:

- Chưa có công cụ nhập toàn bộ dữ liệu MySQL/PHP. Ciphertext/session cũ không tương thích; cần mapping và thử cutover riêng.
- Chưa có tự động telemetry/log/trace activity, cảnh báo gửi mail bất thường và customer 360 như nguồn.
- Certificate scan tối đa 200 certificate mỗi vòng; tls_days là hạn gần nhất trong các certificate đã quan sát, cờ scan_complete cho biết đã đọc hết hay chưa. Số liệu không đọc được hiển thị unknown.
- Một số màn hình quản trị mới vẫn có nhãn tiếng Việt khi chọn English; catalog có đầy đủ nhưng chưa áp dụng hết vào mọi nhãn mới.
- Chưa có toàn bộ thao tác edit trong UI cho profile/admin phụ/nhóm/alias và các wizard của PHP.
- Mailbox/recovery có retrieve nhưng retention cloud của hai nhóm này chưa tự prune theo keep_cloud; cần chính sách lifecycle ở storage.
- Cloud dùng một remote rclone cấu hình cho mỗi policy thay vì wizard credential từng nhà cung cấp. Credentials do người vận hành quản lý ngoài repo.
- Migration chỉ chuyển mapping sau khi xác minh, không tự chép mail. Recovery chỉ xác nhận backend staged/config applied; full restore phải diễn tập và ghi biên bản thực tế.
- Web không tự thay binary Rust như self-update PHP; cập nhật bằng image/binary đã build và kiểm thử.
- Archive mailbox/recovery tối đa 256 MiB và được mã hóa trong RAM; dùng pipeline snapshot khác cho dữ liệu lớn. Backup panel cũng cần RAM phù hợp.

## Kiểm chứng và giới hạn

`cargo test --locked --all-targets -j4` chạy 13 unit và 20 PostgreSQL integration tests.
Integration dùng DB riêng qua SQLx; role local cần CREATEDB.
`tests/browser.mjs` kiểm tra login/dashboard/tạo gói, mười trang admin nâng cao, trial công khai,
trang chủ desktop/mobile và lỗi JavaScript. Chạy trên DB browser thử nghiệm mới,
không chạy trên DB production vì fixture tạo admin và gói.

Các fixture dùng native HTTP API và CLI giả lập có chủ đích. Vandelay thật,
Stalwart CLI thật, rclone cloud, SMTP, DNS/domain mail và SePay ngân hàng thật chưa
được kiểm chứng vì chưa có cấu hình/credential dịch vụ tương ứng. Không suy ra
“production ready” từ suite này. Dockerfile đã cập nhật dependency nhưng chưa có
bằng chứng build production image trong vòng kiểm chứng này.
