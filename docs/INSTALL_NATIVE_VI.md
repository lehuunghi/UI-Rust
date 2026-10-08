# Cài UI-Rust native trên Ubuntu 24.04 / Debian 12 từ A–Z

Hướng dẫn này dành cho VPS, máy vật lý hoặc VM Linux chạy **systemd**, không dùng Docker. PostgreSQL chạy trên cùng máy; Nginx và HTTPS được cấu hình theo [hướng dẫn tổng thể](INSTALL_VI.md). Không cần PHP, MySQL hoặc Node.js để chạy ứng dụng.

Các lệnh bên dưới là hướng dẫn để chạy trên **máy đích của bạn**, bằng tài khoản có `sudo`. Chúng chưa được thực thi toàn bộ trên máy production. Mã ứng dụng, PostgreSQL và các luồng kiểm thử đã được kiểm tra trong môi trường Linux đám mây; điều đó không thay thế kiểm thử Stalwart, SMTP, SePay và restore trên hạ tầng thực.

> **Chọn đúng nguồn:** lấy mã từ nhánh `main` của [lehuunghi/UI-Rust](https://github.com/lehuunghi/UI-Rust), kiểm tra và ghi lại commit trước khi triển khai. Production nên giữ đúng commit đã kiểm thử để dựng lại cùng release. Có thể dùng bộ `ui-rust-source.tar.gz` và `SHA256SUMS` từ nguồn tin cậy nếu cần chuyển nguồn offline. Không dùng database MySQL của bản PHP làm `DATABASE_URL` cho bản Rust.

## 1. Chuẩn bị máy

Ví dụ xuyên suốt:

| Thành phần | Giá trị ví dụ |
| --- | --- |
| Hệ điều hành | Ubuntu 24.04 hoặc Debian 12 |
| Tên miền panel | `panel.example.com` — thay bằng tên miền của bạn |
| HTTP nội bộ | `127.0.0.1:8080` |
| PostgreSQL | phiên bản 17, `127.0.0.1:5432` |
| Database / role | `ui_rust` / `ui_rust` |
| User Linux | `ui-rust`, không đăng nhập shell |
| Binary và assets | `/opt/ui-rust/releases/<release>/` |
| Release đang dùng | `/opt/ui-rust/current` |
| Dữ liệu ghi được | `/var/lib/ui-rust/` |
| Cấu hình bí mật | `/etc/ui-rust/ui-rust.env` |

Chuẩn bị DNS A/AAAA của tên miền panel trỏ về máy này. Chỉ tạo AAAA nếu IPv6 thực sự hoạt động. Không trỏ MX của mail về panel: panel là ứng dụng quản trị; Stalwart là dịch vụ mail riêng.

Máy build cần thêm RAM và dung lượng cho Cargo; nếu VPS nhỏ, build trên một máy Linux cùng kiến trúc và ABI phù hợp rồi chuyển release sang VPS. Binary build trên Ubuntu 24.04 không mặc nhiên chạy trên Debian 12 vì phiên bản glibc có thể mới hơn. Cách dễ nhất là build ngay trên đúng hệ điều hành/kiến trúc đích; hoặc dùng image Docker đi kèm.

Kiểm tra trước:

```bash
cat /etc/os-release
uname -m
df -h /
free -h
timedatectl status
```

Đồng bộ thời gian phải hoạt động để TOTP, phiên đăng nhập và webhook có timestamp hoạt động đúng. Nếu máy đã có PostgreSQL, Nginx hoặc dịch vụ đang dùng cổng 8080/5432, kiểm tra cấu hình hiện tại trước; hướng dẫn này giả định máy mới và không yêu cầu xóa dịch vụ có sẵn.

## 2. Cài gói nền tảng

```bash
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  ca-certificates curl gnupg git build-essential pkg-config libssl-dev \
  openssl python3 nginx rclone
```

`build-essential` phục vụ build Rust và SQLite bundled. `openssl` phục vụ tạo khóa/DKIM; `rclone` chỉ được sử dụng khi bạn bật backup cloud. Node.js/Playwright chỉ cần cho kiểm thử trình duyệt, không phải dependency runtime.

## 3. Cài PostgreSQL 17 từ PGDG chính thức

Kho mặc định của Ubuntu 24.04 và Debian 12 không cung cấp cùng phiên bản PostgreSQL 17. Thêm kho chính thức PGDG với `signed-by`; không dùng `trusted=yes`, không tắt kiểm tra chữ ký apt.

Khối này chỉ chấp nhận đúng hai hệ điều hành trong tiêu đề. Fingerprint khóa PGDG đang dùng là `B97B0AFCAA1A47F044F244A07FCC7D46ACCC4CF8`. Nếu PostgreSQL công bố thay khóa, kiểm tra thông báo chính thức và cập nhật fingerprint; không bỏ qua kiểm tra để cài tiếp.

```bash
(
  set -eu
  . /etc/os-release
  case "$ID:$VERSION_ID" in
    ubuntu:24.04) UI_PG_SUITE=noble-pgdg ;;
    debian:12) UI_PG_SUITE=bookworm-pgdg ;;
    *) printf '%s\n' 'Chỉ dùng khối này trên Ubuntu 24.04 hoặc Debian 12.' >&2; exit 1 ;;
  esac
  UI_PG_KEY_TMP=$(mktemp)
  trap 'rm -f "$UI_PG_KEY_TMP"' EXIT
  curl --proto '=https' --tlsv1.2 -fsS --connect-timeout 10 --max-time 60 \
    https://www.postgresql.org/media/keys/ACCC4CF8.asc \
    -o "$UI_PG_KEY_TMP"
  UI_PG_FINGERPRINT=$(gpg --show-keys --with-colons "$UI_PG_KEY_TMP" \
    | awk -F: '$1 == "fpr" { print $10; exit }')
  test "$UI_PG_FINGERPRINT" = B97B0AFCAA1A47F044F244A07FCC7D46ACCC4CF8
  sudo install -m 0644 "$UI_PG_KEY_TMP" /usr/share/keyrings/postgresql-pgdg.asc
  sudo tee /etc/apt/sources.list.d/ui-rust-pgdg.sources >/dev/null <<EOF
Types: deb
URIs: https://apt.postgresql.org/pub/repos/apt
Suites: $UI_PG_SUITE
Components: main
Signed-By: /usr/share/keyrings/postgresql-pgdg.asc
EOF
  sudo apt-get update
  sudo apt-get install -y --no-install-recommends postgresql-17 postgresql-client-17
)
```

Xác minh:

```bash
pg_lsclusters
sudo -u postgres psql --port=5432 --dbname=postgres --command='SHOW server_version;'
/usr/lib/postgresql/17/bin/pg_dump --version
/usr/lib/postgresql/17/bin/pg_restore --version
```

Cluster được dùng phải là phiên bản 17 và online. Nếu máy đã có cluster cũ chiếm cổng 5432, PGDG có thể tạo cluster 17 trên cổng khác: chỉnh các bước tiếp theo theo cổng thực tế hoặc lên kế hoạch di chuyển; không dừng/xóa cluster cũ một cách tùy tiện.

Không mở PostgreSQL ra Internet. Trên máy mới, giữ `listen_addresses` ở localhost và xác minh rule loopback trong `pg_hba.conf` dùng `scram-sha-256`. Tệp mặc định của cluster 17/main nằm dưới `/etc/postgresql/17/main/`; tên cluster có thể khác trên máy của bạn.

## 4. Tạo role và database riêng

Lệnh đầu hỏi mật khẩu tương tác; mật khẩu không xuất hiện trong command line hoặc shell history. Lưu mật khẩu trong trình quản lý mật khẩu để nhập lại khi tạo cấu hình ở bước 7. Chọn mật khẩu ngẫu nhiên mạnh, ví dụ tối thiểu 24 ký tự.

```bash
sudo -u postgres createuser --port=5432 \
  --pwprompt --no-superuser --no-createdb --no-createrole ui_rust
sudo -u postgres createdb --port=5432 --owner=ui_rust ui_rust
psql --host=127.0.0.1 --port=5432 --username=ui_rust --dbname=ui_rust \
  --password --command='SELECT current_user, current_database();'
```

Role này là owner của database panel, đủ quyền DDL để chạy migrations. Ứng dụng hiện chạy migrations khi khởi tạo `serve`, `worker` và các lệnh dùng database; vì vậy không đổi sang một runtime role thiếu quyền DDL mà chưa thiết kế quy trình migration phù hợp. Tài khoản kiểm thử integration cần quyền tạo database và phải tách khỏi role production.

## 5. Lấy đúng mã nguồn và build Rust 1.99.0

Thực hiện phần build bằng tài khoản SSH bình thường; không chạy Cargo bằng root.

### 5.1. Lấy nguồn từ GitHub main

Clone vào thư mục mới, kiểm tra commit thực tế và tình trạng source trước khi build:

```bash
mkdir -p "$HOME/build"
git clone --branch main --single-branch \
  https://github.com/lehuunghi/UI-Rust.git "$HOME/build/UI-Rust"
cd "$HOME/build/UI-Rust"
git rev-parse HEAD
git status --short
```

Lưu SHA do `git rev-parse HEAD` trả về cùng hồ sơ triển khai. Nhánh `main` có thể thay đổi theo thời gian; để cài lại một release đã kiểm thử, checkout commit đã lưu thay vì lấy bản mới một cách tự động. Thay placeholder bằng SHA thật trước khi chạy:

```bash
git checkout --detach 'COMMIT_40_HEX_DA_DUYET'
git rev-parse HEAD
```

### 5.2. Hoặc dùng bộ nguồn có checksum

Chuyển `ui-rust-source.tar.gz` và `SHA256SUMS` sang cùng một thư mục trên máy đích, qua kênh bạn tin cậy, chẳng hạn `scp`. Chạy từ thư mục chứa hai tệp:

```bash
sha256sum --check SHA256SUMS
mkdir -p "$HOME/build/UI-Rust"
tar --extract --gzip --file=ui-rust-source.tar.gz --directory="$HOME/build/UI-Rust"
cd "$HOME/build/UI-Rust"
test -f Cargo.toml
test -f Cargo.lock
test -d src
```

Giải nén vào thư mục trống. SHA256SUMS bảo đảm tệp khớp bản bàn giao; hãy lấy cả archive và manifest từ nguồn tin cậy. Archive có `Cargo.toml` ngay tại root, không cần `--strip-components`. Ghi lại checksum và commit nguồn được cung cấp cùng archive, nếu có.

Chỉ chọn một trong hai cách lấy nguồn. Ghi lại checksum archive hoặc commit triển khai để có thể dựng lại đúng release.

### 5.3. Cài Rust vào tài khoản build

Tải installer rustup qua HTTPS từ nguồn chính thức. Có thể xem nội dung script trước khi thực thi. Không tắt xác minh TLS hoặc dùng mirror không kiểm soát.

```bash
(
  set -eu
  UI_RUSTUP_SCRIPT=$(mktemp)
  trap 'rm -f "$UI_RUSTUP_SCRIPT"' EXIT
  curl --proto '=https' --tlsv1.2 -fsS --connect-timeout 10 --max-time 120 \
    https://sh.rustup.rs -o "$UI_RUSTUP_SCRIPT"
  sh "$UI_RUSTUP_SCRIPT" -y --profile minimal --default-toolchain 1.99.0 --no-modify-path
)
export PATH="$HOME/.cargo/bin:$PATH"
rustup component add --toolchain 1.99.0 rustfmt
rustc +1.99.0 --version
cargo +1.99.0 --version
```

Build từ thư mục chứa `Cargo.toml`:

```bash
cargo +1.99.0 fmt --all --check
cargo +1.99.0 test --locked --lib -j2
cargo +1.99.0 build --locked --release -j2
./target/release/ui-rust keygen >/dev/null
ldd ./target/release/ui-rust
```

`--locked` sử dụng đúng dependency trong `Cargo.lock`. `-j2` giới hạn số job build; giảm thành `-j1` nếu máy thiếu RAM. Nếu compiler bị kill/OOM, tăng tài nguyên hoặc build ở máy khác tương thích; không chạy lặp lại vô hạn. Không chạy integration tests với `DATABASE_URL` production.

## 6. Tạo user dịch vụ và cài release

Các lệnh sau dành cho lần cài đầu tiên. Nếu user/thư mục đã tồn tại, kiểm tra owner và mục đích trước khi dùng lại.

```bash
sudo useradd --system --user-group \
  --home-dir /var/lib/ui-rust --shell /usr/sbin/nologin ui-rust
sudo install -d -o root -g root -m 0755 /opt/ui-rust /opt/ui-rust/releases
sudo install -d -o root -g ui-rust -m 0750 /etc/ui-rust
sudo install -d -o ui-rust -g ui-rust -m 0750 /var/lib/ui-rust
sudo install -d -o ui-rust -g ui-rust -m 0700 \
  /var/lib/ui-rust/backups /var/lib/ui-rust/private
```

Vẫn ở thư mục source đã build, tạo release có tên riêng:

```bash
UI_RELEASE=$(date -u +%Y%m%dT%H%M%SZ)
UI_RELEASE_DIR="/opt/ui-rust/releases/$UI_RELEASE"
sudo install -d -o root -g root -m 0755 "$UI_RELEASE_DIR"
sudo install -o root -g root -m 0755 target/release/ui-rust "$UI_RELEASE_DIR/ui-rust"
sudo cp -a static "$UI_RELEASE_DIR/static"
sudo chown -R root:root "$UI_RELEASE_DIR/static"
sudo chmod -R u=rwX,go=rX "$UI_RELEASE_DIR/static"
sudo ln -sfnT "$UI_RELEASE_DIR" /opt/ui-rust/current
sudo ln -s /opt/ui-rust/current/static /var/lib/ui-rust/static
```

Binary và static assets thuộc root, user dịch vụ chỉ đọc. `locales` và migrations đã được nhúng vào binary khi build, nên runtime không cần bản rời của chúng. `static` vẫn cần ở WorkingDirectory: nếu chỉ chép binary, trang HTML có thể trả về nhưng CSS/JS sẽ thiếu.

Mọi tiến trình sẽ dùng `/var/lib/ui-rust` làm WorkingDirectory. Nhờ đó `backups` tương đối của CLI và thư mục static có vị trí thống nhất giữa web/worker. Cấu hình `BACKUP_DIRECTORY` cho UI được đặt cùng thư mục backup này ở bước tiếp theo.

## 7. Tạo cấu hình bí mật

### 7.1. Tạo file lần đầu, không in khóa/mật khẩu

Script này yêu cầu một phiên SSH có terminal. Nó hỏi tên miền và mật khẩu PostgreSQL, tự percent-encode mật khẩu trong URL và tự sinh APP_KEY ngẫu nhiên 32 byte. File tạo bằng `O_EXCL`: nếu file đã tồn tại, script dừng để tránh ghi đè APP_KEY.

```bash
sudo python3 <<'PY'
import getpass
import os
import re
import secrets
from urllib.parse import quote, urlsplit

with open('/dev/tty', 'r+') as tty:
    tty.write('APP_URL HTTPS, ví dụ https://panel.example.com: ')
    tty.flush()
    app_url = tty.readline().strip().rstrip('/')
    if not re.fullmatch(r'https://[A-Za-z0-9.-]+(?::[0-9]{1,5})?', app_url):
        raise SystemExit('APP_URL phải là origin HTTPS, không path/query/credential.')
    parsed = urlsplit(app_url)
    if parsed.port is not None and not 1 <= parsed.port <= 65535:
        raise SystemExit('Port APP_URL không hợp lệ.')
    db_password = getpass.getpass('Mật khẩu PostgreSQL của role ui_rust: ', stream=tty)
    if not db_password or any(c in db_password for c in '\r\n\0'):
        raise SystemExit('Mật khẩu rỗng hoặc chứa ký tự điều khiển không hợp lệ.')

values = {
    'DATABASE_URL': 'postgres://ui_rust:' + quote(db_password, safe='')
                    + '@127.0.0.1:5432/ui_rust?sslmode=disable',
    'APP_KEY': secrets.token_hex(32),
    'APP_URL': app_url,
    'BIND': '127.0.0.1:8080',
    'RUST_LOG': 'ui_rust=info,tower_http=info',
    'BACKUP_DIRECTORY': '/var/lib/ui-rust/backups',
    'SMTP_HOST': '',
    'SMTP_USER': '',
    'SMTP_PASSWORD': '',
    'SMTP_FROM': '',
    'SEPAY_ACCOUNT_NUMBER': '',
    'SEPAY_BANK_CODE': '',
    'SEPAY_API_KEY': '',
    'SEPAY_HMAC_SECRET': '',
    'SEPAY_USER_API_TOKEN': '',
    'SEPAY_API_MODE': 'live',
    'SEPAY_POLL_SECONDS': '0',
}
path = '/etc/ui-rust/ui-rust.env'
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, 'w') as f:
    os.fchmod(f.fileno(), 0o600)
    for name, value in values.items():
        f.write(name + '=' + value + '\n')
print('Đã tạo /etc/ui-rust/ui-rust.env, quyền root:root 0600; không in secret.')
PY
```

`sslmode=disable` chỉ dành cho PostgreSQL qua loopback trong ví dụ này. Nếu dùng database bên ngoài máy, thay bằng TLS có kiểm chứng CA/hostname, thường `sslmode=verify-full`, rồi kiểm tra cả kết nối SQLx lẫn `pg_dump`/`pg_restore`. Với CA riêng, thêm `sslrootcert` thích hợp cho SQLx và `PGSSLROOTCERT=/đường/dẫn/ca.crt` cho PostgreSQL client; code backup hiện chỉ chuyển tham số query `sslmode` thành biến môi trường client.

### 7.2. Cách đọc và sửa đúng định dạng

```bash
sudoedit /etc/ui-rust/ui-rust.env
sudo stat --format='%U:%G %a %n' /etc/ui-rust/ui-rust.env
```

Ứng dụng có gọi `dotenvy::dotenv()`, tìm `.env` từ WorkingDirectory lên thư mục cha. Trong cách cài này **systemd nạp `/etc/ui-rust/ui-rust.env` bằng `EnvironmentFile`**; không đặt `.env` ở `/var/lib/ui-rust`, `/var/lib`, `/var` hoặc `/`. File 0600 thuộc root: systemd manager đọc file trước khi chạy process bằng user `ui-rust`; user dịch vụ không cần quyền đọc file trực tiếp.

Đừng chạy `source /etc/ui-rust/ui-rust.env`: `EnvironmentFile`, dotenv và shell có quy tắc quote/interpolation khác nhau. Mọi lệnh CLI trong tài liệu dùng `systemd-run` với `EnvironmentFile` nên không phụ thuộc dotenv và không đưa secret vào command line.

Trong `EnvironmentFile`, dùng `NAME=value`, không `export`, không khoảng trắng trước tên biến. Với mật khẩu có khoảng trắng, `$`, `#`, quote hoặc backslash, dùng chuỗi double-quoted theo cú pháp systemd và escape `"`/`\` đúng cách; systemd không thực hiện phép thế `$VARIABLE` trong giá trị. Không dùng một giá trị nhiều dòng. Ví dụ dưới đây là minh họa cú pháp, không phải secret thật:

```ini
SMTP_PASSWORD="example value with $ and # characters"
```

`APP_KEY` phải giữ nguyên giữa web, mọi worker, backup và restore. Sau khi tạo, sao lưu file cấu hình vào kho bí mật mã hóa tách biệt. Không commit file này, không chép dưới `static`, không gửi kèm log. Không sinh lại APP_KEY khi restart/cập nhật: mất khóa sẽ mất khả năng giải mã token, TOTP, job và backup cũ.

## 8. Chạy migration trước khi mở dịch vụ

Mọi lệnh một lần dùng cùng user, WorkingDirectory, PATH và config. PG client được ưu tiên rõ phiên bản 17:

```bash
sudo systemd-run --unit=ui-rust-migrate --wait --pipe --collect \
  --property=User=ui-rust --property=Group=ui-rust \
  --property=WorkingDirectory=/var/lib/ui-rust \
  --property=EnvironmentFile=/etc/ui-rust/ui-rust.env \
  --property=Environment=PATH=/usr/lib/postgresql/17/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  /opt/ui-rust/current/ui-rust migrate
```

Lệnh phải kết thúc thành công. Nếu lỗi kết nối, kiểm tra role, mật khẩu/cổng và `pg_hba.conf`; nếu lỗi schema/migration, giữ log và xử lý nguyên nhân trước khi mở web. Không sửa checksum hoặc đánh dấu migration đã chạy để bỏ qua lỗi.

## 9. Tạo super admin một lần, không để mật khẩu trong history

Không có admin mặc định. Script tạo một config tạm root-only chứa cấu hình gốc và credential admin, hỏi mật khẩu bằng terminal, không in nội dung ra ngoài. Mật khẩu admin tối thiểu 12 byte; nên dùng chuỗi ngẫu nhiên dài hơn.

```bash
sudo python3 <<'PY'
import getpass
import os
import re

with open('/dev/tty', 'r+') as tty:
    tty.write('Email super admin: ')
    tty.flush()
    email = tty.readline().strip()
    if not re.fullmatch(r'[^\s@]+@[^\s@]+\.[^\s@]+', email):
        raise SystemExit('Email không hợp lệ.')
    password = getpass.getpass('Mật khẩu admin: ', stream=tty)
    confirmation = getpass.getpass('Nhập lại mật khẩu: ', stream=tty)
    if password != confirmation or len(password.encode()) < 12:
        raise SystemExit('Mật khẩu chưa khớp hoặc ngắn hơn 12 byte.')
    if any(c in password for c in '\r\n\0'):
        raise SystemExit('Mật khẩu không được chứa newline/NUL.')

def quoted(value):
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"') + '"'

base = open('/etc/ui-rust/ui-rust.env', encoding='utf-8').read()
fd = os.open('/etc/ui-rust/admin-once.env', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, 'w') as f:
    os.fchmod(f.fileno(), 0o600)
    f.write(base.rstrip('\n') + '\n')
    f.write('ADMIN_EMAIL=' + quoted(email) + '\n')
    f.write('ADMIN_PASSWORD=' + quoted(password) + '\n')
print('Đã tạo config admin tạm, chưa tạo tài khoản.')
PY
sudo systemd-run --unit=ui-rust-create-admin --wait --pipe --collect \
  --property=User=ui-rust --property=Group=ui-rust \
  --property=WorkingDirectory=/var/lib/ui-rust \
  --property=EnvironmentFile=/etc/ui-rust/admin-once.env \
  /opt/ui-rust/current/ui-rust create-admin
sudo rm -f /etc/ui-rust/admin-once.env
```

Luôn chạy dòng xóa config tạm, kể cả khi tạo admin thất bại. Nếu email đã tồn tại, không tạo lại bằng cách xóa user trong database; dùng tính năng đổi/khôi phục mật khẩu phù hợp. Sau đăng nhập lần đầu, bật TOTP và bảo đảm bạn còn truy cập ứng dụng xác thực. Bật email OTP sau khi đã xác nhận SMTP gửi được.

## 10. Tạo systemd service cho web và các worker

Một template dùng chung cho `serve`, `worker`, `mailbox-worker`, `recovery-worker`. Bật web và worker thường trước; hai worker backup mail là tùy chọn.

```bash
sudo tee /etc/systemd/system/ui-rust@.service >/dev/null <<'UNIT'
[Unit]
Description=UI-Rust (%i)
Wants=network-online.target
After=network-online.target postgresql.service

[Service]
Type=simple
User=ui-rust
Group=ui-rust
WorkingDirectory=/var/lib/ui-rust
EnvironmentFile=/etc/ui-rust/ui-rust.env
Environment=PATH=/usr/lib/postgresql/17/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ExecStart=/opt/ui-rust/current/ui-rust %i
Restart=on-failure
RestartSec=5s
KillSignal=SIGINT
TimeoutStopSec=90s
UMask=0077
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/ui-rust
CapabilityBoundingSet=
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6

[Install]
WantedBy=multi-user.target
UNIT
sudo systemd-analyze verify /etc/systemd/system/ui-rust@.service
sudo systemctl daemon-reload
sudo systemctl enable --now ui-rust@serve.service ui-rust@worker.service
```

`serve` không chạy worker trong cùng tiến trình. Không bật instance `ui-rust@migrate`/`ui-rust@backup`: các lệnh một lần dùng `systemd-run`. `KillSignal=SIGINT` phù hợp handler graceful shutdown của chương trình; nếu dừng khi thao tác mail/recovery đang chạy, kiểm tra job bị ngắt/uncertain trước khi thử lại.

Template chặn ghi ngoài `/var/lib/ui-rust` và chặn truy cập home người dùng. Các công cụ backup bổ sung nên đặt tại `/opt/ui-rust/tools`, config/cache rclone tại `/var/lib/ui-rust/private`, không dùng binary hoặc config trong `/root`/`/home`. Nếu adapter recovery cần ghi ở một vùng staging khác, thêm drop-in `ReadWritePaths` chỉ cho đúng vùng đó; giữ dữ liệu nguồn snapshot ở chế độ đọc.

## 11. Kiểm tra trước khi bật HTTPS

```bash
sudo systemctl is-active ui-rust@serve.service ui-rust@worker.service
curl --fail --silent --show-error --max-time 10 http://127.0.0.1:8080/healthz
curl --fail --silent --show-error --max-time 10 http://127.0.0.1:8080/readyz
curl --fail --silent --show-error --max-time 10 --output /dev/null http://127.0.0.1:8080/assets/app.js
sudo journalctl -u ui-rust@serve.service -u ui-rust@worker.service --since=-10min --no-pager
```

`healthz` chỉ xác nhận tiến trình HTTP; `readyz` kiểm tra PostgreSQL. Chúng không chứng minh SMTP/Stalwart/SePay hoặc restore đã thành công. Worker phải active và không có lỗi lặp lại trong log. Không gửi log có token/mật khẩu cho người khác.

Tiếp tục phần Nginx, TLS, DNS và firewall tại [INSTALL_VI.md](INSTALL_VI.md). Với native, upstream proxy là `http://127.0.0.1:8080`. Chỉ mở 80/443 cho panel, giữ 8080 và PostgreSQL nội bộ; không chặn SSH đang sử dụng. `APP_URL` phải khớp origin HTTPS người dùng truy cập để cookie Secure và Origin/CSRF hoạt động.

Sau khi HTTPS hoạt động, mở `https://panel.example.com/dang-nhap`, đăng nhập bằng admin vừa tạo và bật TOTP. Kiểm tra trang chủ, assets và màn hình quản trị trước khi thêm dữ liệu thật.

## 12. Cấu hình SMTP, Stalwart và thanh toán

Nhập thông số SMTP thực trong file cấu hình bằng `sudoedit`, rồi restart web/worker để nạp lại biến. SMTP hiện dùng **STARTTLS cổng 587**; không có biến tùy chọn port/TLS trong Config hiện tại. SMTP phải dùng certificate hợp lệ và credential có quyền gửi từ `SMTP_FROM`.

```bash
sudo systemctl restart ui-rust@serve.service ui-rust@worker.service
```

Kiểm tra một luồng email thật đến hộp thư do bạn quản lý, ví dụ quên mật khẩu, trước khi bật email OTP hoặc duyệt trial công khai cho khách mới. Không yêu cầu người dùng bật email OTP nếu SMTP chưa hoạt động. Admin TOTP không phụ thuộc SMTP.

Thêm Stalwart tại màn hình **Server Stalwart** bằng HTTPS với certificate hợp lệ; dùng token có đúng quyền JMAP cần thiết. Xem cấu hình, dry-run/live và checklist thực tế trong [INSTALL_VI.md](INSTALL_VI.md) và [DEPLOYMENT.md](DEPLOYMENT.md). Dùng database thử riêng cho dry-run; không chuyển dữ liệu đã mô phỏng thành live chỉ bằng việc bỏ cờ dry-run.

SePay webhook/API là tùy chọn. `SEPAY_POLL_SECONDS=0` tắt polling. Chỉ đặt account/bank/API key/HMAC/user API token thực sau khi thống nhất chế độ live/sandbox; không dùng giao dịch ngân hàng thật để thử replay tùy tiện. Xem luồng thử và hạn chế trong hướng dẫn tổng thể.

## 13. Backup PostgreSQL panel

### 13.1. Tạo backup thủ công

Kiểm tra dư RAM và dung lượng trước: backup PostgreSQL hiện được mã hóa trong RAM. Panel backup chứa dữ liệu PostgreSQL, không chứa mailbox/blob/backend Stalwart hoặc APP_KEY.

```bash
sudo systemd-run --unit=ui-rust-backup --wait --pipe --collect \
  --property=User=ui-rust --property=Group=ui-rust \
  --property=WorkingDirectory=/var/lib/ui-rust \
  --property=EnvironmentFile=/etc/ui-rust/ui-rust.env \
  --property=Environment=PATH=/usr/lib/postgresql/17/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  /opt/ui-rust/current/ui-rust backup
sudo -u ui-rust ls -lh /var/lib/ui-rust/backups
```

CLI ghi `backups/ui-<UTC>-<random>.pgenc`, quyền 0600. Backup qua UI dùng thư mục con namespace của database trong `BACKUP_DIRECTORY`; không đổi tên/xóa file mà UI đang quản lý. Các archive tạo bằng CLI không tự trở thành bản ghi trong lịch sử backup UI.

### 13.2. Lịch và cloud

Worker thường xử lý lịch/retention/retry của backup panel. Trong UI cấu hình số bản local/cloud và đích rclone đã được thiết lập, ví dụ `offsite:ui-rust-panel`. Rclone remote phải được cấu hình cho user dịch vụ, không chỉ cho tài khoản SSH.

```bash
sudo install -d -o ui-rust -g ui-rust -m 0700 /var/lib/ui-rust/private/rclone
sudo -u ui-rust env RCLONE_CONFIG=/var/lib/ui-rust/private/rclone/rclone.conf rclone config
sudo chmod 0600 /var/lib/ui-rust/private/rclone/rclone.conf
```

Thêm dòng này vào `/etc/ui-rust/ui-rust.env`, rồi restart các service đang chạy:

```ini
RCLONE_CONFIG=/var/lib/ui-rust/private/rclone/rclone.conf
```

OAuth token của một số remote có thể được rclone cập nhật nên config cần writable bởi `ui-rust`. Chọn quyền remote tối thiểu phù hợp upload/retrieve/retention; giữ lifecycle cloud riêng cho archive mailbox/recovery như phần giới hạn hiện tại. Sao lưu APP_KEY ở nơi khác, tránh đặt cạnh archive với cùng quyền truy cập.

### 13.3. Diễn tập restore vào database riêng

Không restore thử vào database đang phục vụ panel. Ví dụ tạo `ui_rust_restore_test`, dùng lại owner nhưng cấu hình riêng. Không chạy worker trên database restore thử, vì dữ liệu có thể chứa endpoint/token/jobs trỏ Stalwart hoặc SMTP thật.

```bash
sudo -u postgres createdb --port=5432 --owner=ui_rust ui_rust_restore_test
sudo python3 <<'PY'
import os
from urllib.parse import urlsplit, urlunsplit

lines = open('/etc/ui-rust/ui-rust.env', encoding='utf-8').read().splitlines()
updated = []
found = False
for line in lines:
    if line.startswith('DATABASE_URL='):
        value = line.split('=', 1)[1]
        if value.startswith(('"', "'")):
            raise SystemExit('Ví dụ này cần DATABASE_URL không quote như bước 7; chỉnh bằng sudoedit nếu khác.')
        parsed = urlsplit(value)
        value = urlunsplit((parsed.scheme, parsed.netloc, '/ui_rust_restore_test', parsed.query, parsed.fragment))
        line = 'DATABASE_URL=' + value
        found = True
    updated.append(line)
if not found:
    raise SystemExit('Thiếu DATABASE_URL.')
fd = os.open('/etc/ui-rust/restore-test.env', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, 'w') as f:
    os.fchmod(f.fileno(), 0o600)
    f.write('\n'.join(updated) + '\n')
print('Đã tạo config restore test; không khởi chạy worker trên config này.')
PY
```

Thay đường dẫn placeholder bên dưới bằng một file `.pgenc` thực mà user `ui-rust` đọc được. Không chạy placeholder nguyên trạng:

```bash
sudo systemd-run --unit=ui-rust-restore-test --wait --pipe --collect \
  --property=User=ui-rust --property=Group=ui-rust \
  --property=WorkingDirectory=/var/lib/ui-rust \
  --property=EnvironmentFile=/etc/ui-rust/restore-test.env \
  --property=Environment=PATH=/usr/lib/postgresql/17/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  /opt/ui-rust/current/ui-rust restore \
  '/var/lib/ui-rust/backups/<TEN_BACKUP_THUC>.pgenc' --confirm-db ui_rust_restore_test
sudo -u postgres psql --port=5432 --dbname=ui_rust_restore_test \
  --command='SELECT count(*) AS users FROM users; SELECT count(*) AS domains FROM domains; SELECT count(*) AS mailboxes FROM email_accounts;'
```

CLI restore kiểm tra APP_KEY/tính toàn vẹn, sau đó `pg_restore --clean --if-exists` thay object trong một transaction. Tên `--confirm-db` phải trùng database trong URL. So sánh schema/migration, số dòng, hóa đơn/thanh toán, quota và mapping Stalwart với dữ liệu gốc. Xác nhận checksum archive không đồng nghĩa đã chứng minh khôi phục thành công.

Sau diễn tập, giữ biên bản và xóa config thử khi không cần. Chỉ xóa database thử sau khi xác nhận chính xác tên và hoàn tất kiểm tra; không dùng lệnh `dropdb` chung cho production.

```bash
sudo rm -f /etc/ui-rust/restore-test.env
```

## 14. Worker mailbox/recovery tùy chọn

Không cần bật phần này để chạy panel cơ bản. Cài Vandelay và Stalwart CLI từ nguồn phát hành chính thức, xác minh checksum/chữ ký, đúng kiến trúc và tương thích schema phiên bản Stalwart của bạn. Không lấy executable fixture trong tests để dùng production.

Đặt executable operator-owned tại ví dụ `/opt/ui-rust/tools/vandelay` và `/opt/ui-rust/tools/stalwart-cli`, root sở hữu và executable bởi service user. Sau đó thêm đường dẫn vào cấu hình:

```ini
MAILBOX_VANDELAY_BIN=/opt/ui-rust/tools/vandelay
RECOVERY_STALWART_CLI_BIN=/opt/ui-rust/tools/stalwart-cli
RECOVERY_SOURCES_FILE=/etc/ui-rust/recovery-sources.json
```

`RECOVERY_SOURCES_FILE` là cấu hình do người vận hành quản lý, không phải file công khai. User `ui-rust` phải đọc được nó: dùng root:ui-rust 0640 và thư mục cha 0750. Snapshot nguồn cần consistent marker và đầy đủ data/blob/bootstrap; backend đang ghi không trở thành snapshot nhất quán chỉ bằng việc đặt `consistent=true`. Quyền đọc file snapshot phải được cấp theo group/ACL, không mở toàn bộ dữ liệu mail cho mọi user. Đích diễn tập phải cô lập outbound và tách khỏi mail server thật. Xem JSON nguồn, marker và contract adapter trong [CONFIG_SERVICES_VI.md](CONFIG_SERVICES_VI.md) trước khi bật; đổi đường dẫn container của ví dụ thành đường dẫn native thực tế.

`restore_root` phải nằm ngoài WorkingDirectory `/var/lib/ui-rust`, ví dụ `/srv/ui-rust-rehearsal`. Tạo đúng thư mục thật và cấp ngoại lệ ghi cho **recovery-worker** vì template mặc định có `ProtectSystem=strict`. Thay đường dẫn nếu bạn chọn vùng staging khác, đồng thời dùng chính đường dẫn đó trong JSON:

```bash
sudo install -d -o ui-rust -g ui-rust -m 0700 /srv/ui-rust-rehearsal
sudo install -d -o root -g root -m 0755 \
  /etc/systemd/system/ui-rust@recovery-worker.service.d
sudo tee /etc/systemd/system/ui-rust@recovery-worker.service.d/paths.conf >/dev/null <<'UNIT'
[Service]
ReadWritePaths=/srv/ui-rust-rehearsal
UNIT
sudo systemctl daemon-reload
```

Thêm `ReadWritePaths` cho đúng thư mục snapshot nếu adapter được giao tạo checkpoint tại đó; giữ nguồn snapshot đã chuẩn bị ở chế độ đọc nếu không có nhu cầu này. Backend lab riêng cần quyền/ACL phù hợp cho adapter; ngoại lệ systemd không tự cấp quyền filesystem và không tự cô lập outbound. Không đặt staging trong `static`, `backups` hoặc thư mục code panel.

Khi công cụ và nguồn đã sẵn sàng, chạy một vòng **trước khi đã tạo lịch/job thực** để kiểm tra worker kết nối và cấu hình. Nếu có job pending, `*-once` sẽ thực thi job đó; nó không phải một chế độ dry-run chung.

```bash
sudo systemd-run --unit=ui-rust-mailbox-once --wait --pipe --collect \
  --property=User=ui-rust --property=Group=ui-rust \
  --property=WorkingDirectory=/var/lib/ui-rust \
  --property=EnvironmentFile=/etc/ui-rust/ui-rust.env \
  /opt/ui-rust/current/ui-rust mailbox-once
sudo systemd-run --unit=ui-rust-recovery-once --wait --pipe --collect \
  --property=User=ui-rust --property=Group=ui-rust \
  --property=WorkingDirectory=/var/lib/ui-rust \
  --property=EnvironmentFile=/etc/ui-rust/ui-rust.env \
  /opt/ui-rust/current/ui-rust recovery-once
sudo systemctl enable --now ui-rust@mailbox-worker.service ui-rust@recovery-worker.service
```

Mọi worker phải dùng cùng APP_KEY, database, backup directory và rclone config. Archive mailbox/recovery hiện giới hạn 256 MiB và xử lý trong RAM. Với dữ liệu lớn, dùng pipeline snapshot/backup backend phù hợp; không coi archive này là bản backup toàn hệ thống không giới hạn. Phần restore vật lý cần adapter thực và xác nhận mailbox/folder/attachment/queue trên môi trường cô lập; xem [PARITY.md](PARITY.md).

## 15. Cập nhật bằng release mới

Không cập nhật binary đang dùng bằng `cp` ghi đè, không kéo `main` trực tiếp trên production rồi tự build vô điều kiện. Build và kiểm tra bản nguồn mới như bước 5, cài vào một thư mục release mới như bước 6 **nhưng chưa đổi symlink**.

Quy trình trong cửa sổ bảo trì:

1. Ghi lại release hiện tại bằng `readlink -f /opt/ui-rust/current`, commit/checksum, APP_KEY và cấu hình.
2. Kiểm tra migration của bản mới; chạy thử nâng cấp trên database restore riêng. Không sửa migration đã áp dụng.
3. Dừng web và mọi worker đang bật để dừng thao tác mới, đặc biệt thao tác sync/restore đang chạy. Chờ hoặc xử lý trạng thái job bị ngắt.
4. Dùng **binary cũ** tạo backup PostgreSQL sau khi dừng dịch vụ; sao lưu config/APP_KEY và đưa bản backup ra storage khác.
5. Đổi symlink sang release mới, chạy `migrate` bằng lệnh bước 8, rồi bật lại đúng các service trước đó.
6. Kiểm tra readyz, assets, login, worker/log và một thao tác ít rủi ro; quan sát lỗi sync/email trước khi kết thúc bảo trì.

Ví dụ thao tác dừng dịch vụ cơ bản:

```bash
sudo systemctl stop ui-rust@serve.service ui-rust@worker.service
# Chỉ dừng hai instance này nếu bạn đã bật chúng:
sudo systemctl stop ui-rust@mailbox-worker.service ui-rust@recovery-worker.service
```

Chạy backup như bước 13.1 khi symlink còn trỏ binary cũ. Sau đó thay `<THU_MUC_RELEASE_MOI_DA_CAI>` bằng đường dẫn thực:

```bash
sudo ln -sfnT '/opt/ui-rust/releases/<THU_MUC_RELEASE_MOI_DA_CAI>' /opt/ui-rust/current
# Chạy lại migrate ở bước 8; chỉ khởi động khi migrate thành công.
sudo systemctl start ui-rust@serve.service ui-rust@worker.service
# Chỉ khởi động nếu hai worker này nằm trong cấu hình vận hành của bạn:
sudo systemctl start ui-rust@mailbox-worker.service ui-rust@recovery-worker.service
```

APP_KEY và `/var/lib/ui-rust` không thay khi cập nhật. Static symlink sẽ đi theo release mới. Nếu sửa file EnvironmentFile, restart tất cả instance đang chạy để cùng nạp cấu hình.

## 16. Rollback và chuyển máy

**Rollback binary chỉ an toàn khi binary cũ tương thích schema và dữ liệu sau migration mới.** Đổi symlink không hoàn tác migration. Chương trình tự chạy migrations lúc khởi động; không dùng binary cũ để đoán xem schema mới có chạy được hay không.

Nếu chưa chạy migration hoặc đã chứng minh tương thích, dừng dịch vụ, đổi symlink về release trước, rồi khởi động/kiểm tra. Nếu schema không tương thích, cần restore backup trước nâng cấp vào database đã chuẩn bị, dùng APP_KEY tương ứng và đối chiếu các thay đổi Stalwart/thanh toán ngoài database xảy ra sau backup. Restore có thể mất các giao dịch phát sinh từ thời điểm backup; lập kế hoạch bảo trì và đối soát trước.

Chuyển sang một VPS khác cần:

- Release nguồn/binary và static phù hợp hệ điều hành/kiến trúc đích.
- PostgreSQL backup đã thử restore, APP_KEY gốc và cấu hình secret chuyển qua kênh an toàn.
- Rclone config, binary Vandelay/Stalwart CLI và nguồn snapshot/adapter nếu đang dùng.
- DNS/TLS, firewall, SMTP outbound, khả năng truy cập Stalwart/SePay và thời gian đồng bộ.
- User/service/path/quyền file giống hướng dẫn; chạy migration/readyz và diễn tập các dịch vụ ngoài.

Dừng worker ở máy cũ trước khi bật worker ở máy mới để tránh thao tác ngoài/notification đồng thời. Khi đổi endpoint/config/version, kiểm tra job pending/failed/uncertain và preview cũ; không phát lại thao tác không rõ kết quả. Nếu chuyển bản PHP, xem [MIGRATION.md](MIGRATION.md): đây là schema mới PostgreSQL, không phải import tự động database MySQL.

Chuyển **toàn bộ** kho backup cùng hồ sơ artifact trong PostgreSQL. Nếu hostname/port/dbname trong DATABASE_URL thay đổi, namespace archive UI cũng thay đổi: làm theo phần rehome của [CONFIG_SERVICES_VI.md](CONFIG_SERVICES_VI.md) khi web/worker đều dừng. Native dùng owner `ui-rust:ui-rust` thay UID 10001 của ví dụ Docker. Giữ bản cũ cho rollback; không dùng symlink nối namespace và không chỉ chép các file `.pgenc` ở root rồi bỏ qua `mailboxes`/`recovery` bên trong namespace.

## 17. Xử lý lỗi thường gặp

| Triệu chứng | Kiểm tra / xử lý |
| --- | --- |
| `APP_KEY required`, `DATABASE_URL required` | Lệnh chạy ngoài systemd chưa được nạp EnvironmentFile. Dùng `systemd-run` như tài liệu, không in config ra terminal. |
| `APP_KEY must contain 32 bytes` | Cần đúng 64 ký tự hex; lấy lại khóa gốc nếu đã có dữ liệu, không tạo khóa mới để chữa lỗi. |
| PostgreSQL authentication failed | Role/password, cổng thật, DB URL đã percent-encode và pg_hba scram. Test psql hỏi password tương tác. |
| Migration permission/checksum error | Owner/DDL role, đúng database và đúng source/migrations. Không bỏ qua lỗi/checksum. |
| `502 Bad Gateway` | `ui-rust@serve`, cổng BIND, journal và proxy upstream. Test readyz tại loopback. |
| Trang trắng / JS-CSS 404 | `/var/lib/ui-rust/static` symlink, static cùng phiên bản binary và WorkingDirectory. |
| Login/POST bị CSRF hoặc cookie không giữ | APP_URL đúng origin HTTPS, proxy Host và URL browser; không dùng domain/port khác. |
| Hóa đơn không kích hoạt / email không gửi | Worker active, job state, SMTP/SePay/Stalwart thực; xem trạng thái uncertain trước khi retry. |
| Backup báo thiếu pg_dump/pg_restore | PATH service/transient unit có `/usr/lib/postgresql/17/bin`; client không cũ hơn server. |
| Rclone không thấy remote | RCLONE_CONFIG đúng, user ui-rust đọc/ghi được config; config trong tài khoản SSH khác không tự dùng được. |
| Worker recovery không đọc snapshot | Quyền traverse thư mục/ACL, ProtectHome, path tuyệt đối không symlink và marker nhất quán. Không chmod toàn bộ mail data 0777. |
| Build bị kill / disk full | RAM, swap policy, dung lượng Cargo; giảm jobs hoặc dùng build host tương thích. |

Lệnh xem trạng thái gọn:

```bash
sudo systemctl status ui-rust@serve.service ui-rust@worker.service --no-pager
sudo journalctl -u ui-rust@serve.service -u ui-rust@worker.service -n 100 --no-pager
curl --fail --silent --show-error --max-time 10 http://127.0.0.1:8080/readyz
```

Trước khi đưa khách thật vào, hoàn tất checklist vận hành/kiểm thử dịch vụ ngoài ở [INSTALL_VI.md](INSTALL_VI.md). Các tính năng có fixture/mocks kiểm thử chưa chứng minh cấu hình mail/payment/restore production của bạn hoạt động.
