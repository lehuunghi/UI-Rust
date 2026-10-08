# Cấu hình và kết nối dịch vụ cho UI-Rust

Tài liệu này bổ sung cho [hướng dẫn cài đặt từ A–Z](INSTALL_VI.md). Các tên biến,
đường dẫn và hợp đồng recovery bên dưới được đối chiếu với mã nguồn hiện tại.
`example.com`, IP tài liệu và chuỗi `THAY_...` là ví dụ; thay bằng thông tin của bạn.
Khi cài từ [GitHub](https://github.com/lehuunghi/UI-Rust), dùng tài liệu nằm trong
commit bạn đã chọn; lưu SHA cùng cấu hình vận hành. Gói source/image cung cấp
riêng là snapshot, không tự cập nhật theo `main`.

## 1. Những cấu hình phải giữ giống nhau

Web, worker và các công cụ backup của **cùng một panel** phải dùng chung
`DATABASE_URL`, `APP_KEY`, `APP_URL` và kho backup. Trên nhiều máy, mount cùng kho
backup tại cùng đường dẫn bên trong tiến trình/container. Không dùng một volume
Docker cục bộ khác nhau ở mỗi máy rồi coi đó là kho dùng chung.

`APP_KEY` là 32 byte, biểu diễn bằng **64 ký tự hex**. Tạo một lần bằng
`ui-rust keygen` hoặc `openssl rand -hex 32`, lưu trong tệp môi trường riêng có quyền
`0600`, và sao lưu khóa tách biệt với backup dữ liệu. Đừng tạo khóa mới khi chuyển
VPS, cập nhật image hoặc thêm worker. Khóa này giải mã token Stalwart, mật khẩu tác
vụ, TOTP, hồ sơ trial và archive; giữ database mà mất khóa vẫn mất khả năng đọc dữ
liệu mã hóa. Hiện chưa có quy trình xoay khóa tự động.

Mỗi tiến trình có pool tối đa 20 kết nối PostgreSQL. Khi bật cả web, worker,
mailbox-worker và recovery-worker, tính cả bốn pool trong ngân sách kết nối DB;
thêm replica sẽ tăng số kết nối tối đa tương ứng.

`migrate`, `create-admin`, `serve` và các worker đều gọi migration khi khởi động.
Chạy `ui-rust migrate` trước khi nâng phiên bản, rồi khởi động các tiến trình.
SQLx quản lý khóa migration trên PostgreSQL, nhưng vẫn nên dùng một bước triển khai
migration để dễ quan sát lỗi. Không sửa migration đã chạy. Không có biến
`SKIP_MIGRATIONS` hay `RUN_MIGRATIONS` trong phiên bản này; nếu tách role triển khai
và role runtime, phải kiểm chứng quyền cần cho migration của runtime.

## 2. Bảng biến môi trường đầy đủ

Ứng dụng đọc `.env` trong **WorkingDirectory** nếu có. Biến môi trường đã được tiến
trình nhận có ưu tiên cao hơn `.env`. Với Docker Compose, `env_file: .env` truyền
biến vào container; với systemd, dùng `EnvironmentFile`. Thay đổi tệp môi trường
cần restart các tiến trình liên quan.

### Biến nền tảng

| Biến | Mặc định | Cách sử dụng |
|---|---|---|
| `DATABASE_URL` | Không có; bắt buộc | PostgreSQL URL, ví dụ `postgres://ui:THAY_MAT_KHAU@db:5432/ui`. Percent-encode ký tự đặc biệt của username/password. Có thể dùng `?sslmode=require` nếu DB hỗ trợ TLS. |
| `APP_KEY` | Không có; bắt buộc | Khóa 64 ký tự hex; giống nhau trên mọi tiến trình của panel. |
| `APP_URL` | `http://localhost:8080` | URL HTTPS bên ngoài, ví dụ `https://panel.example.com`. Dùng origin gốc, không đặt panel dưới subpath. Scheme HTTPS làm cookie có thuộc tính Secure. |
| `BIND` | `127.0.0.1:8080` | Địa chỉ HTTP nội bộ. Docker image đặt `0.0.0.0:8080`; proxy/publish port quyết định truy cập từ ngoài. |
| `RUST_LOG` | `ui_rust=info,tower_http=info` | Mức log, do tracing đọc; cấu hình production có thể giữ mặc định. |
| `ADMIN_EMAIL` | Không có | Chỉ dùng cho lệnh `create-admin`; email hợp lệ, được lưu dạng chữ thường. |
| `ADMIN_PASSWORD` | Không có | Chỉ dùng cho `create-admin`; tối thiểu 12 byte. Xóa biến sau khi tạo admin. Lệnh không phải thao tác upsert để chạy lặp lại. |

Trong Compose của repo, `POSTGRES_PASSWORD` là biến **của Compose/PostgreSQL**, không
phải biến Rust đọc trực tiếp. Compose dùng nó để tạo `DATABASE_URL` cho ứng dụng.
Nếu dùng mật khẩu trong mẫu Compose, chuỗi hex ngẫu nhiên tránh vấn đề ký tự đặc biệt
trong URL. Thay đổi `POSTGRES_PASSWORD` trong `.env` không tự đổi mật khẩu role của
database đã có volume; phải đổi role DB và cấu hình ứng dụng đồng bộ.

### SMTP

| Biến | Mặc định | Cách sử dụng |
|---|---|---|
| `SMTP_HOST` | Rỗng | Host SMTP relay, ví dụ `smtp.example.com`; không gồm scheme hay `:port`. Rỗng nghĩa là không gửi mail. |
| `SMTP_USER` | Rỗng | Username do relay cấp. |
| `SMTP_PASSWORD` | Rỗng | Mật khẩu SMTP hoặc app password. |
| `SMTP_FROM` | Rỗng | Địa chỉ người gửi hợp lệ, ví dụ `UI Panel <noreply@example.com>`. |

Ứng dụng dùng **STARTTLS trên cổng 587**. Chưa có `SMTP_PORT`, `SMTP_TLS` hay chế độ
SMTPS 465 qua biến môi trường; relay chỉ hỗ trợ 465 cần một relay tương thích 587
hoặc thay đổi mã nguồn có kiểm thử.

### SePay

| Biến | Mặc định | Cách sử dụng |
|---|---|---|
| `SEPAY_API_KEY` | Rỗng | Shared API key xác thực webhook bằng `Authorization: Apikey ...`. |
| `SEPAY_HMAC_SECRET` | Rỗng | Secret HMAC webhook; nếu có, được ưu tiên và webhook không dùng cơ chế API key nữa. |
| `SEPAY_ACCOUNT_NUMBER` | Rỗng | Tài khoản nhận tiền; phải khớp payload ngân hàng. |
| `SEPAY_BANK_CODE` | Rỗng | Mã ngân hàng dùng trong hiển thị thanh toán/QR; lấy đúng mã SePay sử dụng. |
| `SEPAY_USER_API_TOKEN` | Rỗng | Bearer token của **User API**, khác shared API key webhook. |
| `SEPAY_API_MODE` | `live` | Chỉ nhận `live` hoặc `sandbox`; chi phối tác vụ nhập từ User API. |
| `SEPAY_POLL_SECONDS` | `0` | `0` tắt nhập định kỳ; giá trị khác 0 được giới hạn thực tế trong 300–86400 giây. Worker kéo lại 7 ngày gần nhất, chống ghi trùng. |

### Backup và recovery tùy chọn

| Biến | Mặc định | Cách sử dụng |
|---|---|---|
| `BACKUP_DIRECTORY` | `backups` | Gốc kho backup của UI/worker; nên dùng đường dẫn tuyệt đối, ví dụ `/app/backups` trong container. |
| `BACKUP_RCLONE_BIN` | `rclone` | Executable rclone cho backup **PostgreSQL của panel**: upload/retrieve/retention. Có thể dùng đường dẫn tuyệt đối. Mailbox/recovery hiện gọi `rclone` trong `PATH`, không dùng override này. |
| `MAILBOX_VANDELAY_BIN` | `vandelay` | Executable Vandelay cho mailbox-worker. Binary không có sẵn trong Docker image mặc định. |
| `RECOVERY_STALWART_CLI_BIN` | `stalwart-cli` | Executable Stalwart CLI cho recovery-worker. Binary không có sẵn trong image mặc định. |
| `RECOVERY_SOURCES_FILE` | Không đặt | JSON do operator quản lý, khai báo nguồn snapshot và đích diễn tập. Không đặt thì danh sách nguồn recovery rỗng. |

`ui-rust backup` ghi vào **`backups` dưới WorkingDirectory**, không đọc
`BACKUP_DIRECTORY`. Backup qua UI/worker có thêm thư mục namespace theo host, port
và tên DB trong `DATABASE_URL`; mailbox và recovery nằm bên dưới namespace đó.
Đừng giả định đường dẫn CLI và UI giống nhau. Khi chuyển host/port DB, giữ nguyên
cả kho backup cũ và hồ sơ của nó; namespace có thể thay đổi. Restore CLI có thể
nhận đường dẫn archive cụ thể trên kho cũ.

Các biến như `RCLONE_CONFIG` được **rclone** đọc và kế thừa từ tiến trình, không
được Rust diễn giải. `VANDELAY_PASSWORD`, `STALWART_URL`, `STALWART_TOKEN`,
`STALWART_USER`, `STALWART_PASSWORD`, `NO_COLOR` và `UI_RECOVERY_*` được ứng dụng
đặt cho subprocess khi chạy công cụ; không cần lưu credential tương ứng thêm
một lần trong `.env` của panel.

Ví dụ các cấu hình tùy chọn có thể thêm vào tệp môi trường của bạn:

```dotenv
# SMTP: thay bằng tài khoản relay thật trước khi bật email OTP.
SMTP_HOST=smtp.example.com
SMTP_USER=noreply@example.com
SMTP_PASSWORD=THAY_APP_PASSWORD
SMTP_FROM="UI Panel <noreply@example.com>"

# Webhook API key; để HMAC rỗng nếu phía gửi không hỗ trợ contract HMAC bên dưới.
SEPAY_API_KEY=THAY_SHARED_WEBHOOK_KEY
SEPAY_HMAC_SECRET=
SEPAY_ACCOUNT_NUMBER=THAY_SO_TAI_KHOAN
SEPAY_BANK_CODE=THAY_MA_NGAN_HANG

# Chưa bật User API/polling cho đến khi đã thử nghiệm tài khoản của bạn.
SEPAY_USER_API_TOKEN=
SEPAY_API_MODE=live
SEPAY_POLL_SECONDS=0

BACKUP_DIRECTORY=/app/backups
BACKUP_RCLONE_BIN=/usr/bin/rclone
RCLONE_CONFIG=/run/ui-rust/rclone.conf
MAILBOX_VANDELAY_BIN=/run/ui-rust-tools/vandelay
RECOVERY_STALWART_CLI_BIN=/run/ui-rust-tools/stalwart-cli
RECOVERY_SOURCES_FILE=/run/ui-rust/recovery-sources.json
```

## 3. Stalwart: live, dry-run và quyền API

Stalwart là mail server riêng; cài panel không tự cài Stalwart hay tạo mailbox
server. Cài mail server theo tài liệu chính thức tại <https://stalw.art/docs/>,
cấu hình certificate/DNS/lưu trữ, rồi thêm server trong màn hình **Server Stalwart**.

Nhập tên, URL gốc, credential, trạng thái active, primary và dry-run. Nên dùng
`https://mail.example.com` với certificate hợp lệ và hostname chính xác. URL không
chứa username/password, query hay fragment. Runtime không đi theo redirect để gửi
credential sang nơi khác; reverse proxy Stalwart phải phục vụ trực tiếp endpoint.

| Dạng credential nhập trong UI | Authorization được gửi |
|---|---|
| Token thông thường, ví dụ `API_...` | `Bearer <token>` |
| `user:password` | `Basic <base64(user:password)>` |
| `basic:<base64(user:password)>` | `Basic <phần base64>` |

Không nhập chuỗi `Bearer ...` hoặc `Basic ...` đã có prefix HTTP vào trường token;
ứng dụng tự tạo header. Credential được mã hóa trong PostgreSQL bằng `APP_KEY`.
Với token có dấu `:` và không bắt đầu `API_`, parser hiểu đó là Basic credential.

URL gốc thông thường được gọi tại `/jmap/`. Nếu URL kết thúc `/api`, ứng dụng gọi
nguyên endpoint đó; nếu kết thúc `/jmap`, ứng dụng thêm dấu `/`. Metadata quyền và
schema vẫn lấy ở `/api/account` và `/api/schema` trên cùng origin. Khả năng quản trị
dùng extension `urn:stalwart:jmap`, không chỉ JMAP mailbox thông thường; hãy thử
đúng phiên bản/schema Stalwart của bạn trước khi chuyển production.

Tạo credential riêng cho panel. Quyền phải khớp tính năng bạn bật. Các thao tác
quản trị kiểm tra quyền remote mới trước execute, ví dụ `sysDomainQuery`,
`sysDomainGet`, `sysDomainCreate`, `sysDomainUpdate`, `sysAccountQuery`,
`sysAccountGet`, `sysAccountCreate`, `sysAccountUpdate`. Tác vụ loại khác cần quyền
`sys<LoạiObject><ThaoTác>` tương ứng. Không có một danh sách quyền tối thiểu duy
nhất cho cả sync, báo cáo, certificate, queue, tenant và recovery. Xác minh quyền
thực tế qua metadata/UI; lỗi 403 phải được xử lý bằng quyền phù hợp, không bỏ kiểm
tra quyền.

`dry_run=1` mô phỏng write và trả ID bắt đầu `dry-`; query mô phỏng rỗng. Nó không
chứng minh token/schema live hoạt động. Dùng DB và server thử riêng cho dry-run;
không tạo dữ liệu mô phỏng rồi đổi cùng cấu hình đó thành live. Với nhiều server,
domain đã tạo được gắn server cố định. Primary/default theo khách ảnh hưởng domain
mới; chuyển mail cũ cần quy trình migration riêng.

Sau khi thêm live server, kiểm tra metadata/health, tạo một domain và mailbox thử,
đợi sync thành công, kiểm tra remote và thử gửi/nhận. Nếu job `failed`, `uncertain`
hoặc `unconfirmed`, đọc trạng thái remote trước khi retry để tránh tạo trùng.
Thay URL/token/server config làm tăng version, có thể làm kế hoạch/job cũ không
hợp lệ; tạo preview mới sau khi đối chiếu.

## 4. DNS, certificate và các cổng mạng

Dùng hai hostname dễ phân biệt:

| Tên ví dụ | Mục đích |
|---|---|
| `panel.example.com` | UI-Rust qua reverse proxy HTTPS. A/AAAA trỏ VPS/proxy của panel. |
| `mail.example.com` | Stalwart API/JMAP/SMTP/IMAP. A/AAAA trỏ mail server, certificate chứa hostname này. |
| `customer.example` | Domain email khách hàng; MX/SPF/DKIM/DMARC theo cấu hình Stalwart thực tế. |

Không bật proxy HTTP/CDN thông thường cho hostname dùng SMTP/IMAP; dịch vụ cần
định tuyến đúng các giao thức đó. PTR/rDNS do nhà cung cấp IP/VPS thiết lập, không
phải bản ghi tự tạo trong zone DNS của khách.

| Luồng | Cổng thường dùng | Ai cần truy cập |
|---|---|---|
| Internet → reverse proxy panel | TCP 443; TCP 80 cho redirect/ACME nếu dùng | Người dùng, webhook SePay, cấp certificate |
| Proxy → UI-Rust | TCP 8080 mặc định | Chỉ proxy/nội bộ; Compose chỉ publish loopback |
| UI/worker → PostgreSQL | TCP 5432 mặc định | Chỉ ứng dụng/mạng private; không cần public DB |
| UI/worker → Stalwart API/JMAP | TCP 443 hoặc cổng HTTPS đã cấu hình | Các tiến trình dùng API và công cụ recovery/mailbox |
| Worker → SMTP relay thông báo | TCP 587 STARTTLS | Gửi OTP/reset/thông báo |
| Mail server ↔ mail server | TCP 25 | Stalwart gửi/nhận email; panel kiểm tra STARTTLS tới mail host ở cổng này |
| Email client → Stalwart | TCP 587/465 và 993/143 tùy cấu hình Stalwart | Mail client; không phải port panel tự mở |
| UI/worker → resolver DoH | TCP 443 tới `cloudflare-dns.com` | TXT ownership, MX/SPF/DKIM/DMARC/PTR |
| UI/worker → SePay | TCP 443 tới `userapi.sepay.vn` hoặc `userapi-sandbox.sepay.vn` | Nhập User API tùy mode |
| Trình duyệt → QR SePay | TCP 443 tới `qr.sepay.vn` | Hiển thị QR thanh toán |
| Worker → cloud storage | HTTPS/port của remote rclone | Backup offsite |

Đồng bộ giờ qua NTP để TOTP, HMAC 5 phút, certificate và snapshot age hoạt động
đúng. Panel kiểm tra DNS qua **Cloudflare DNS-over-HTTPS** hiện chưa có biến đổi
resolver. Mạng chỉ cho DNS nội bộ mà chặn HTTPS resolver sẽ gây lỗi/unknown; DNS
split-horizon riêng sẽ không xuất hiện trong kết quả public resolver.

Trong UI domain, mở hướng dẫn DNS để lấy TXT ownership dạng:

```dns
_ui-verification.customer.example. IN TXT "ui-verification=TOKEN_DO_PANEL_CAP"
```

Sau xác minh, sao chép MX/SPF/DKIM/DMARC từ zone/hướng dẫn của server. Không dùng
một DKIM key ví dụ cho mọi khách. Nếu thêm DMARC, có thể bắt đầu policy quan sát
phù hợp kế hoạch vận hành rồi tăng policy sau khi kiểm tra nguồn gửi. Trang kiểm
tra mail DNS tách `passed`, `failed` và `unknown`; `unknown` cần xác minh bổ sung.
STARTTLS ở đây kiểm tra **cổng 25**, độc lập với SMTP thông báo port 587.

## 5. SMTP, OTP và thư thông báo

1. Cấp một mailbox/app password tại relay; đặt đúng bốn biến SMTP.
2. Kiểm tra DNS/certificate của relay và outbound TCP 587 từ máy/container worker.
3. Khởi động `worker`, kiểm tra mẫu email cần dùng đang bật trong **Email templates**.
4. Thử quên mật khẩu bằng một tài khoản thử và xác minh nhận thư/liên kết đúng
   `APP_URL`. Theo dõi queue gửi mail và log worker, không chỉ `/readyz`.
5. Bật TOTP cho admin và lưu thông tin phục hồi theo quy trình của bạn. Chỉ bật email
   OTP khi đã kiểm chứng relay.

Worker gửi từng thông báo, tối đa 5 lần với khoảng chờ 2 phút khi gửi lỗi. Worker
phải chạy để OTP và reset được gửi. OTP có hiệu lực 5 phút; link đặt lại mật khẩu
30 phút. Domain gửi `SMTP_FROM` cần SPF/DKIM/DMARC theo relay để tránh spam. Khi
SMTP chưa cấu hình, các thông báo thông thường có thể được bỏ qua, nhưng các luồng
cần OTP/reset không thể hoàn tất gửi mail. Duyệt trial của khách mới cần SMTP và
mẫu `password_reset` bật để gửi liên kết tự đặt mật khẩu.

## 6. SePay: webhook và User API

### Webhook

Khai báo endpoint **POST**:

```text
https://panel.example.com/sepay/webhook
```

GET endpoint chỉ trả thông tin endpoint, không kiểm chứng một khoản thanh toán.
Với xác thực API key, cấu hình phía gửi truyền đúng:

```http
Authorization: Apikey THAY_SHARED_WEBHOOK_KEY
Content-Type: application/json
```

Nếu đặt `SEPAY_HMAC_SECRET`, phía gửi hoặc một gateway của bạn phải hỗ trợ đúng
hợp đồng sau; đừng bật HMAC chỉ vì có chỗ nhập secret nếu SePay không gửi đúng
header/thuật toán này:

```text
X-SePay-Timestamp: <Unix seconds>
X-SePay-Signature: sha256=<hex HMAC-SHA256(secret, timestamp + '.' + raw_body)>
```

`raw_body` là byte JSON nhận nguyên vẹn, không parse/serialize lại. Timestamp được
chấp nhận trong ±300 giây. Nếu cả key và HMAC cùng được đặt, **chỉ HMAC được chấp
nhận**. Proxy không được bỏ header xác thực hay sửa body.

Phải điền đúng tài khoản nhận, ngân hàng và nội dung thanh toán do hóa đơn tạo.
Payload kiểm tra với ID `0` không ghi ledger. Khoản unmatched/late/additional cần
đối soát; late không tự kích hoạt dịch vụ. Kiểm tra hóa đơn, payment, ledger và
subscription sau một giao dịch thật nhỏ do bạn chủ động thực hiện.

### User API và sandbox

User API dùng `SEPAY_USER_API_TOKEN`. Bật thủ công từ trang thanh toán hoặc đặt
`SEPAY_POLL_SECONDS=600` để worker nhập định kỳ. API cố định theo mode:

```text
live:    https://userapi.sepay.vn/v2/transactions
sandbox: https://userapi-sandbox.sepay.vn/v2/transactions
```

Nhập thủ công chọn khoảng tối đa 31 ngày; mỗi lần giới hạn 20 trang × 100 giao dịch
và có thời hạn thực thi. Nếu kết quả `complete=false`, tiếp tục đối soát/nhập phạm
vi hẹp hơn; các khoản đã nhận vẫn được giữ. Sandbox tách namespace và **không ghi
ledger/kích hoạt dịch vụ**. `SEPAY_API_MODE=sandbox` chỉ chi phối User API; không
biến webhook thành webhook sandbox. Thử sandbox bằng DB/panel thử, tránh cấu hình
webhook ngân hàng thật vào môi trường kiểm thử.

## 7. Rclone và kho backup dùng chung

Docker image hiện chứa `rclone`, `openssl`, `pg_dump` và `pg_restore` PostgreSQL 17.
Host native cần tự cài các công cụ đó. Client PostgreSQL phải hỗ trợ phiên bản
server; ví dụ `pg_dump` 17 không dump server PostgreSQL 18.

Cấu hình remote rclone trên máy vận hành bằng `rclone config`; chọn provider thực
của bạn như S3/R2 hoặc Drive theo tài liệu rclone. Giữ credential ngoài Git. Ví dụ
đặt tên remote là `offsite`, rồi nhập policy destination trong UI:

```text
offsite:ui-rust/prod-panel-01
```

Destination phải là remote đã cấu hình, không phải URL HTTP tùy ý. Khi upload,
ứng dụng lưu cloud path; retrieve chỉ nhận lại cloud path đã ghi còn khớp policy
hiện tại. Đổi remote/prefix có thể làm bản cũ không retrieve được qua UI; giữ cấu
hình cũ hoặc dùng quy trình operator với archive/APP_KEY.

### Quyền filesystem của container

Image chạy user `app`, UID **10001**. Tạo tệp/mount mà UID này đọc được; backup và
thư mục diễn tập cần ghi được. Ví dụ trên host, sau khi đã có rclone config thật:

```bash
sudo install -d -o 10001 -g 10001 -m 0700 /etc/ui-rust-integrations
sudo install -d -o 10001 -g 10001 -m 0700 /srv/ui-rust-backups
sudo install -o 10001 -g 10001 -m 0600 ./rclone.conf \
  /etc/ui-rust-integrations/rclone.conf
```

Tệp rclone có thể được mount read-only với remote tĩnh như access key S3. Với
remote OAuth có token cần refresh, kiểm tra rclone có cần ghi cập nhật config;
cấp một config private writable cho worker thay vì một mount read-only gây lỗi
refresh. Không mount config secret vào `/app/static`.

Một overlay Compose cho **rclone** có thể viết như sau, chạy cùng `compose.yaml`:

```yaml
# compose.integrations.yaml
x-rclone: &rclone
  environment:
    RCLONE_CONFIG: /run/ui-rust/rclone.conf
  volumes:
    - /etc/ui-rust-integrations/rclone.conf:/run/ui-rust/rclone.conf:ro

services:
  web:
    <<: *rclone
  worker:
    <<: *rclone
  mailbox-worker:
    <<: *rclone
  recovery-worker:
    <<: *rclone
```

Compose merge giữ volume `panel_backups:/app/backups` và environment DB của file
gốc. Kiểm tra kết quả bằng `docker compose -f compose.yaml -f compose.integrations.yaml
config --quiet` trước khi restart. Nếu render cấu hình đầy đủ, output có thể chứa
secret từ `.env`; không đưa output đó vào ticket/log công khai.

Thử đọc remote từ chính worker với quyền production:

```bash
docker compose -f compose.yaml -f compose.integrations.yaml \
  exec worker rclone lsd offsite:ui-rust
```

Sau đó tạo một backup thử qua UI, chờ upload thành công, download/restore vào DB
thử và kiểm tra. `lsd` thành công chưa chứng minh quyền ghi/retrieve/delete.
Policy panel có lịch 1–8760 giờ, giữ local 1–1000 bản và cloud 1–10000 bản.
Mailbox/recovery lịch 1–168 giờ, giữ local 1–90 bản; retention cloud của hai nhóm
này hiện chưa tự prune theo `keep_cloud`, cần chính sách lifecycle riêng ở storage.

Backup PostgreSQL chỉ chứa DB panel. Cần backup độc lập `APP_KEY`, tệp môi trường,
rclone config, binary/commit triển khai và data/blob/config/keys của Stalwart.
Archive mailbox/recovery và download/retrieve qua UI có giới hạn 256 MiB; dữ liệu
được mã hóa trong RAM. Với mail server lớn, dùng pipeline snapshot/backup của
backend thay vì chia giả hoặc bỏ giới hạn kiểm tra của panel.

### Giữ archive khi thay địa chỉ database hoặc chuyển nơi cài

Namespace của backup UI là **16 ký tự đầu của SHA256** trên chuỗi
`<hostname>:<port><URL path>`. Không có mật khẩu trong chuỗi này. Với
`postgres://ui:...@db:5432/ui`, chuỗi được hash là `db:5432/ui`; port không khai báo
được hiểu là 5432. Dùng hostname/path đã parse theo URL, giữ URL path mã hóa nếu
có; APP_URL và APP_KEY không tham gia hash.

1. Dừng web cùng `worker`, `mailbox-worker`, `recovery-worker` trong thời gian
   chuyển và restore. Sao lưu **toàn bộ kho backup/volume**, không chỉ file `.pgenc`;
   namespace cũ có thể chứa `mailboxes/`, `recovery/` và archive đã đăng ký.
2. Restore DB panel và giữ nguyên APP_KEY theo hướng dẫn chuyển máy. Hồ sơ artifact
   trong DB và tệp vật lý đều phải được giữ; riêng APP_KEY không tự tìm lại folder cũ.
3. Docker → Docker giữ service DB `db`, port 5432, database `ui` thường giữ cùng
   namespace. Khi chuyển sang managed DB/hostname/port/dbname khác, tính namespace
   đích và **sao chép** toàn bộ nội dung namespace cũ sang namespace mới trước khi
   restart. Giữ bản cũ cho rollback; không đổi filename hay metadata archive.
4. Nếu namespace đích đã có dữ liệu, dừng và đối chiếu trước khi merge, tránh ghi
   đè archive. Không dùng symlink để nối thư mục cũ vì các luồng backup/recovery
   có kiểm tra symlink.
5. Kiểm tra UID/quyền, download một archive panel/mailbox/recovery đã có bằng UI,
   đối chiếu checksum và diễn tập restore ở môi trường riêng. Chỉ xóa bản copy cũ
   sau khi đã nghiệm thu và có bản offsite độc lập.

Ví dụ tính tên **không in DATABASE_URL hay mật khẩu**; thay hostname/port/path
trong hai chuỗi bằng cấu hình của bạn:

```bash
ui_old_namespace=$(printf '%s' 'db:5432/ui' | sha256sum | cut -c1-16)
ui_new_namespace=$(printf '%s' 'new-db.example.com:5432/ui' | sha256sum | cut -c1-16)
printf 'Old namespace: %s\nNew namespace: %s\n' "$ui_old_namespace" "$ui_new_namespace"
```

Sau khi web/workers đã dừng, DB đã restore và kho backup host đã chép vào
`/srv/ui-rust-backups`, ví dụ rehome cho container UID 10001:

```bash
if [ "$ui_old_namespace" = "$ui_new_namespace" ]; then
  printf 'Namespace giữ nguyên; không cần rehome.\n'
elif [ ! -d "/srv/ui-rust-backups/$ui_old_namespace" ] \
  || [ -e "/srv/ui-rust-backups/$ui_new_namespace" ]; then
  printf 'Dừng: nguồn thiếu hoặc đích đã tồn tại; cần đối chiếu.\n' >&2
else
  sudo install -d -o 10001 -g 10001 -m 0700 \
    "/srv/ui-rust-backups/$ui_new_namespace" \
  && sudo cp -a "/srv/ui-rust-backups/$ui_old_namespace/." \
    "/srv/ui-rust-backups/$ui_new_namespace/" \
  && sudo chown -R 10001:10001 "/srv/ui-rust-backups/$ui_new_namespace"
fi
```

Kiểm tra kết quả trước khi restart; đây là thao tác bảo trì trên **bản copy đã giữ
bản gốc**. Với named volume Docker, mount/export
volume sang một thư mục bảo trì trước khi thao tác, hoặc dùng container bảo trì
truy cập volume; không tự sửa thư mục nội bộ `/var/lib/docker`. Host native dùng
UID user dịch vụ thực tế thay 10001. Tệp CLI `ui-rust backup` nằm ngay dưới
WorkingDirectory `backups/` vẫn phải được chuyển riêng; chúng không thuộc
namespace của backup UI.

## 8. Vandelay và backup mailbox

Cài Vandelay từ nguồn chính thức, chọn đúng OS/kiến trúc và phiên bản tương thích
Stalwart. Kiểm tra checksum/signature theo bản phát hành. Không thay executable
bằng CLI fixture trong `tests`. Kiểm tra `vandelay --help` để xác nhận binary hỗ
trợ giao diện dưới đây trước khi bật lịch.

Ứng dụng gọi công cụ với cấu trúc:

```text
vandelay export --url <JMAP_URL> --auth-basic <email> --account-name <email> <sqlite_path>
vandelay import jmap --url <JMAP_URL> --auth-basic <email> --account-name <email> --dry-run <sqlite_path>
vandelay import jmap --url <JMAP_URL> --auth-basic <email> --account-name <email> <sqlite_path>
```

App password được đặt riêng qua environment `VANDELAY_PASSWORD` của subprocess,
không truyền trên command line. Dùng **app password của mailbox**, không dùng
shared token quản trị server thay nó.

Trong màn hình backup mailbox:

1. Chọn mailbox active, đã sync thật; URL JMAP phải HTTPS và **cùng origin** với
   server đã gắn. Dùng JMAP session URL thực tế do Stalwart phục vụ.
2. Nhập app password, chu kỳ, retention; tạo backup thủ công trước khi bật lịch.
3. Mount executable vào mailbox-worker và đặt `MAILBOX_VANDELAY_BIN`; chạy
   `mailbox-worker` hoặc `mailbox-once` để xử lý một vòng.
4. Xác minh job thành công, SQLite quick_check, archive checksum và offsite.
5. Tạo **mailbox thử khác**, đánh dấu restore target; chạy restore preview.
6. Trong 15 phút, chính admin tạo preview mới được xác nhận `RESTORE` một lần.
   Restore là nhập bổ sung, không có `--prune` để xóa mail đích. Kiểm tra folder,
   message và attachment thực tế sau khi nhập.

Giữ cùng `/app/backups` cho web và mailbox-worker để download/retrieve thấy cùng
tệp. Chạy lại job restore `uncertain` có thể tạo trùng mail; đối chiếu mailbox
trước khi tạo preview mới. Đổi URL/token/server mapping làm profile cũ cần lưu
lại cấu hình và preview mới.

## 9. Recovery Stalwart: source JSON và snapshot nhất quán

Phần này cần operator hiểu backend đang dùng. Mỗi source chỉ là **snapshot đã
được tạo nhất quán**, không phải quyền cho panel tự sao chép một DB đang ghi.
Backend RocksDB/SQLite/PostgreSQL/S3/search có cách checkpoint/export/restore khác
nhau. Dùng công cụ đúng backend; với storage ngoài máy, export snapshot thành các
tệp có thể đọc và khôi phục bằng adapter của bạn.

Tạo JSON ngoài repo/public, ví dụ `/etc/ui-rust-integrations/recovery-sources.json`,
mount vào mọi tiến trình cần đọc nó ở `/run/ui-rust/recovery-sources.json`. Tất cả
đường dẫn bên trong JSON là đường dẫn **nhìn thấy trong container/tiến trình**,
không phải đường dẫn host nếu hai bên mount khác nhau.

### Ví dụ source và đích diễn tập

```json
{
  "profiles": {
    "mail-primary": {
      "server_origin": "https://mail.example.com",
      "snapshot_marker": "/srv/stalwart-snapshots/current/marker.json",
      "config_plan_path": "/srv/stalwart-snapshots/current/configuration.ndjson",
      "components": {
        "data": "/srv/stalwart-snapshots/current/data",
        "blob": "/srv/stalwart-snapshots/current/blob",
        "bootstrap": "/srv/stalwart-snapshots/current/bootstrap"
      },
      "required_components": ["data", "blob", "bootstrap"],
      "max_snapshot_age_seconds": 86400,
      "max_bytes": 250000000,
      "max_files": 10000
    },
    "mail-rehearsal": {
      "server_origin": "https://mail-lab.example.com",
      "restore_root": "/srv/stalwart-rehearsal",
      "isolated_rehearsal": true,
      "outbound_isolated": true,
      "seed_account_ids": ["THAY_ID_ADMIN_LAB"],
      "restore_command": ["/run/ui-rust-tools/backend-adapter", "restore"]
    }
  }
}
```

`mail-primary`/`mail-rehearsal` là source keys để chọn trong UI. Key chỉ chứa chữ,
số, `_` hoặc `-`, dài tối đa 100. `server_origin` phải khớp **scheme + host + port**
của server, không chứa path. Thay `seed_account_ids` bằng ID thật của admin lab
không gắn domain, hoặc dùng `[]` nếu API lab không có Account.

Ví dụ trên dùng snapshot đã chuẩn bị sẵn. Nếu cần operator adapter chuẩn bị
checkpoint trước mỗi lần capture, thêm vào source:

```json
{
  "snapshot_command": ["/run/ui-rust-tools/backend-adapter", "snapshot"]
}
```

Adapter phải tạo checkpoint/export và marker nhất quán; exit code thành công
một mình không chứng minh snapshot đúng. `current` trong ví dụ phải là **thư mục
thật**, không phải symlink trỏ snapshot mới nhất: code từ chối symlink ở cả đường
dẫn và nội dung inventory. Publish một snapshot hoàn chỉnh rồi giữ ổn định suốt
capture; cập nhật JSON sang đường dẫn snapshot có version là lựa chọn rõ ràng hơn
khi cần giữ nhiều snapshot.

### Contract source được kiểm tra

| Trường/giới hạn | Yêu cầu |
|---|---|
| JSON nguồn | Tối đa 256 KiB, có object `profiles`, tối đa 100 profile. |
| Đường dẫn snapshot/plan/marker | Tuyệt đối, tồn tại, không có ancestor symlink; ngoài `static` và `backups` của WorkingDirectory. |
| `components` | 1–12 loại; root không chồng lấp. Nội dung chỉ file/thư mục thường, không symlink, socket, FIFO hay device. |
| Loại component | `data`, `blob`, `search`, `bootstrap`, `queue`, `certificates`, `external_directory`, `lookup`, `metrics`, `traces`, `encryption_keys`, `logs`. |
| `required_components` | Mặc định `data`, `blob`, `bootstrap`; phải có toàn bộ thành phần cần để thực sự phục hồi backend bạn dùng. |
| `max_snapshot_age_seconds` | Mặc định 86400; giới hạn thực tế 300–604800 giây. Marker không được đi trước đồng hồ quá 300 giây. |
| `max_bytes` | Mặc định tối đa 256 MiB; cấu hình cao hơn vẫn bị chặn. Configuration/manifest cũng chiếm dung lượng archive. |
| `max_files` | Mặc định 10000; giới hạn 1–20000. |
| `config_plan_path` | NDJSON UTF-8 tối đa 64 MiB; hash phải khớp `config_sha256` trong marker. |
| Không có `config_plan_path` | CLI lấy config live riêng sau checkpoint; metadata thể hiện `backend-checkpoint-plus-live-management`, không đảm bảo cùng checkpoint config/backend. |

Đừng bớt `required_components` chỉ để vượt một lỗi khi snapshot thiếu blob, key,
directory ngoài hay queue mà mail server cần. Inventory/hash kiểm tra tệp ổn
định không thay thế consistency guarantee của backend.

### Marker của checkpoint

Đây là **mẫu**, không ghi nguyên thời gian/chuỗi hash mẫu rồi đặt `consistent=true`:

```json
{
  "snapshot_id": "snapshot-YYYYMMDDTHHMMSSZ",
  "server_origin": "https://mail.example.com",
  "created_at": "THAY_THOI_GIAN_RFC3339_CUA_CHECKPOINT",
  "consistent": true,
  "config_sha256": "THAY_SHA256_64_KY_TU_HEX_CUA_CONFIGURATION_NDJSON"
}
```

`snapshot_id` chỉ nhận chữ, số, `_`, `.`, `-`, dài 1–100. `created_at` là thời gian
checkpoint thực (RFC3339, ví dụ format `2026-10-08T12:00:00Z`), không đổi thành
"bây giờ" để che snapshot cũ. Lấy SHA256 trên byte NDJSON đã chốt:

```bash
sha256sum /srv/stalwart-snapshots/current/configuration.ndjson
```

Nếu adapter dùng container mount read-only, nó không thể ghi snapshot vào mount
đó. Chọn một trong hai cách: snapshot chuẩn bị ở host rồi mount read-only, hoặc
cho adapter quyền ghi tối thiểu vào vùng snapshot riêng. Không cấp quyền ghi cả
dữ liệu live chỉ để panel backup.

### CLI cấu hình và vault

Cài Stalwart CLI chính thức tương thích server; kiểm tra help/version. Ứng dụng
gọi giao diện:

```text
stalwart-cli snapshot <ObjectType1,ObjectType2,...> --include-secrets --quiet
stalwart-cli apply --file <plan.ndjson> --dry-run --quiet
stalwart-cli apply --file <plan.ndjson> --quiet
```

App truyền URL/credential riêng bằng `STALWART_URL` và `STALWART_TOKEN` hoặc cặp
`STALWART_USER`/`STALWART_PASSWORD`. JSON plan chỉ nhận `@type` upsert/update,
`object` nằm trong phạm vi object type của profile, `value` là object; upsert cần
`matchOn`. Mỗi dòng tối đa 8 MiB, tổng tối đa 100000 operation. Dùng output CLI
thật thay vì tự chế cấu hình theo ví dụ không phù hợp schema server.

Các loại snapshot được chấp nhận: `Account`, `AccountPassword`, `ApiKey`,
`AppPassword`, `Role`, `Tenant`, `Domain`, `Directory`, `DataStore`, `BlobStore`,
`SearchStore`, `InMemoryStore`, `DkimSignature`, `Certificate`, `AcmeProvider`,
`DnsServer`, `NetworkListener`, `SystemSettings`, `Authentication`, `Security`,
`Jmap`, `Imap`, `ReportSettings`, `SenderAuth`, `MtaInboundSession`,
`MtaOutboundStrategy`, `MtaRoute`, `MtaTlsStrategy`, `MtaOutboundThrottle`,
`MtaInboundThrottle`, `MtaQueueQuota`, `SpamSettings`, `SpamRule`, `PublicKey`,
`OAuthClient`. Chọn phạm vi theo khả năng CLI/server và kế hoạch recovery.

Nếu snapshot còn giá trị bị mask bằng `***...`, capture bị từ chối. Có thể bổ sung
vault trong profile UI theo **object type → ID remote chính xác → trường secret**:

```json
{
  "DkimSignature": {
    "THAY_ID_DKIM_REMOTE": {
      "privateKey": "THAY_PRIVATE_KEY_DUNG_CUA_OBJECT"
    }
  }
}
```

Đây chỉ là ví dụ hình dạng; không nhập placeholder vào vault production. Vault
tối đa 1 MiB, chỉ nhận trường có tên thuộc nhóm password/secret/token/credential/
privateKey/accessKey/apiKey/connectionString/dsn/encryptionKey. Trường cấu hình
thường như `name` không được dùng trong vault. Vault được mã hóa bằng `APP_KEY`;
giữ nguồn secret phục hồi tách biệt và bảo vệ archive tương ứng.

## 10. Đích diễn tập và adapter phục hồi backend

Trước khi đánh dấu `isolated_rehearsal`/`outbound_isolated=true`, operator phải
thực sự cô lập đích: host/network riêng; chặn SMTP outbound ra Internet ở firewall
và cloud security group; không trỏ MX production vào lab; không tái dùng credential
cloud DNS/ACME có thể sửa domain thật. Chừa kết nối API panel ↔ lab để quản trị.
Boolean trong JSON là khai báo được code kiểm tra, không phải firewall tự động.

Đích phải khác origin/source, không phải primary và không có domain/account đang
được panel quản lý. Native Domain/query phải rỗng; Account/query tối đa 5 admin
seed khai báo trước, không gắn domain. `restore_root` là thư mục thật đã có,
writable cho UID chạy recovery-worker, ngoài/cũng không bao phủ mã panel, không
chồng lấp bất kỳ component snapshot của profile nào. Tạo thư mục riêng, ví dụ:

```bash
sudo install -d -o 10001 -g 10001 -m 0700 /srv/stalwart-rehearsal
```

Adapter executable phải nằm ngoài WorkingDirectory của panel, là file thật,
không symlink. Command JSON là mảng argv (1–20 phần), không được hiểu như shell
command string. Không dùng `sudo`, `sh -c` để đưa secret vào command line; cấp đúng
quyền backend và executable cho user của worker. Adapter là mã do operator viết
riêng cho backend, không có adapter mẫu giả luôn trả thành công.

Ứng dụng đặt environment cho adapter:

| Biến subprocess | Ý nghĩa |
|---|---|
| `UI_RECOVERY_PHASE` | `snapshot` hoặc `restore`. |
| `UI_RECOVERY_ORIGIN` | Origin của profile đang xử lý; khi restore là origin lab. |
| `UI_RECOVERY_STAGE` | Khi restore: thư mục đã extract và kiểm chứng, chứa `components/...`, `configuration.ndjson`, `manifest.json`; snapshot là chuỗi rỗng. |
| `UI_RECOVERY_RECEIPT` | Đường dẫn để adapter restore ghi receipt JSON. |
| `UI_RECOVERY_ARTIFACT_SHA256` | Hash archive đã đăng ký; receipt phải khớp. Snapshot là chuỗi rỗng. |

Với phase snapshot, adapter chuẩn bị checkpoint/marker rồi thoát `0`. Với restore,
adapter nạp dữ liệu đã stage vào backend của **lab**, khởi động/kiểm tra backend
bằng quy trình phù hợp, thực hiện các kiểm chứng thật và ghi receipt tối đa 32 KiB.
Receipt phải có các trường dưới đây, lấy origin/hash từ environment và chỉ đặt
`true` cho kiểm chứng đã thực hiện:

```json
{
  "artifact_sha256": "GIA_TRI_UI_RECOVERY_ARTIFACT_SHA256",
  "server_origin": "GIA_TRI_UI_RECOVERY_ORIGIN",
  "backend_loaded": true,
  "credentials_checked": true,
  "mail_folders_checked": true,
  "attachments_checked": true,
  "queue_checked": true,
  "outbound_isolated": true
}
```

Không sao chép receipt mẫu thành script luôn trả `true`. Không có
`restore_command` thì panel chỉ stage tệp và apply phần cấu hình lab; nó không
tự nạp RocksDB/PostgreSQL/blob/search/backend ngoài.

Thứ tự hiện tại của restore thật: kiểm tra lab trống → xác minh/archive extract →
CLI apply dry-run → stage vào `restore_root/rehearsal-<job_id>` → adapter restore
nếu có → CLI apply thật. Adapter cần tính đến thứ tự này: kiểm chứng backend trước
configuration apply không thay thế kiểm chứng mail server cuối cùng.

Plan lab chỉ apply `Role`, `Tenant`, `Domain`, `Account`, `DkimSignature`; singleton
và loại khác bị loại khỏi plan diễn tập. Domain được đặt disabled, DNS/DKIM/
certificate management Manual. Đây là diễn tập có kiểm soát, không phải tự thay
toàn bộ cấu hình server production hay tự cutover MX.

## 11. Mount tùy chọn cho các worker mail

Giữ binary cùng thư viện phụ thuộc bên trong image hoặc mount executable static
đúng kiến trúc. Mount riêng một binary dynamic từ distro khác có thể lỗi loader/
glibc dù file có quyền execute. Với binary riêng, xây image kế thừa Dockerfile
đã kiểm tra hoặc mount bộ công cụ tương thích rồi xác minh `--help` từ container.

Overlay **ví dụ**, kết hợp overlay rclone nếu bạn dùng offsite:

```yaml
# compose.mail-tools.yaml
services:
  web:
    environment:
      RECOVERY_SOURCES_FILE: /run/ui-rust/recovery-sources.json
    volumes:
      - /etc/ui-rust-integrations/recovery-sources.json:/run/ui-rust/recovery-sources.json:ro
      - /srv/stalwart-snapshots:/srv/stalwart-snapshots:ro
      - /srv/stalwart-rehearsal:/srv/stalwart-rehearsal
  mailbox-worker:
    environment:
      MAILBOX_VANDELAY_BIN: /run/ui-rust-tools/vandelay
    volumes:
      - /opt/ui-rust-tools:/run/ui-rust-tools:ro
  recovery-worker:
    environment:
      RECOVERY_STALWART_CLI_BIN: /run/ui-rust-tools/stalwart-cli
      RECOVERY_SOURCES_FILE: /run/ui-rust/recovery-sources.json
    volumes:
      - /opt/ui-rust-tools:/run/ui-rust-tools:ro
      - /etc/ui-rust-integrations/recovery-sources.json:/run/ui-rust/recovery-sources.json:ro
      - /srv/stalwart-snapshots:/srv/stalwart-snapshots:ro
      - /srv/stalwart-rehearsal:/srv/stalwart-rehearsal
```

Web cần nhìn thấy nguồn/restore_root để kiểm tra và lưu profile; recovery-worker
cần các đường dẫn tương ứng để thực thi. Nếu còn đặt `RECOVERY_SOURCES_FILE`
chung trong `.env` cho `worker`/`mailbox-worker`, mount tệp đó ở các tiến trình ấy
khi họ cần đọc; hoặc chỉ khai báo biến ở dịch vụ sử dụng như overlay. Source
snapshot adapter cần mount writable riêng nếu nó tạo checkpoint; ví dụ trên chỉ
cho đọc snapshot đã được host chuẩn bị.

Khởi động profile sau khi đã chuẩn bị các tool/config/mount:

```bash
docker compose -f compose.yaml -f compose.integrations.yaml \
  -f compose.mail-tools.yaml --profile mail-recovery config --quiet
docker compose -f compose.yaml -f compose.integrations.yaml \
  -f compose.mail-tools.yaml --profile mail-recovery up -d
```

Compose profile bật **cả mailbox-worker và recovery-worker**. Nếu chỉ cần một,
chỉ định dịch vụ đó ở lệnh `up -d`. `mailbox-once`/`recovery-once` chạy một vòng,
có thể thực hiện job đã chờ; chúng không phải health check read-only. Không chạy
one-shot trên production chỉ để kiểm tra cài binary khi đang có job pending.

## 12. Kiểm chứng phục hồi trước khi dùng production

1. Tạo snapshot nhất quán thực và marker đúng. Capture một archive, kiểm tra
   manifest/inventory/hash, giữ cả APP_KEY và nguồn cấu hình vận hành.
2. Chuẩn bị lab trống, isolation thực; thêm server lab live với credential riêng,
   profile target và mount đúng. Preview recovery và đọc `excluded_lab_types`.
3. Chính admin tạo preview xác nhận `RESTORE_SERVER` trong 15 phút; preview dùng
   một lần, thay profile/config phải preview lại.
4. Đọc kết quả job: `backend_staged` chỉ chứng minh tệp đã được stage;
   `configuration_applied` chỉ chứng minh CLI apply; adapter receipt phản ánh
   contract adapter. `full_restore_verified` không tự biến thành chứng nhận đã
   khôi phục mail toàn phần.
5. Thử đăng nhập mailbox lab, số lượng/thư mục/message/attachment, key giải mã,
   queue không phát ra Internet, backend/search/directory ngoài và tham chiếu
   storage. Kiểm tra tính khôi phục credential theo backend, không chỉ file tồn tại.
6. Ghi biên bản trong UI với bằng chứng ít nhất 20 ký tự và đủ các kiểm chứng.
   `admin_verified` là xác nhận operator; lưu RPO/RTO cùng ghi chú/thời gian, không
   thay kiểm chứng tự động. Sự cố restore `uncertain` cần đối chiếu lab trước khi
   có thao tác mới.

Giới hạn/chức năng chưa tương đương bản PHP được ghi tại [PARITY.md](PARITY.md).
Test fixture CLI/API của repo không chứng minh Vandelay/Stalwart/rclone/SMTP/
SePay thật đã hoạt động với cấu hình của bạn. Chốt nghiệm thu bằng các thử nghiệm
dịch vụ thực ở môi trường thử trước khi nhận người dùng và tiền thật.
