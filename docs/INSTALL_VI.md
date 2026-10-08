# Cài đặt UI-Rust từ A–Z

Bản hướng dẫn cho mã nguồn đang có ngày **08/10/2026**. Panel dùng Rust/Axum,
PostgreSQL và HTML/CSS/JavaScript. Cài panel **không tự cài server mail Stalwart**.

Lấy mã nguồn từ [lehuunghi/UI-Rust](https://github.com/lehuunghi/UI-Rust), ghi lại
commit đã rà soát và dùng cùng commit cho các nơi triển khai. Nhánh `main` cung
cấp bản mới nhất nhưng có thể thay đổi; production nên ghim commit cụ thể.
Gói **ui-rust-source.tar.gz + SHA256SUMS** là lựa chọn chuyển một snapshot nguồn
khi không dùng Git; đối chiếu phiên bản của gói trước khi cài.

## 1. Chọn phương án

| Nơi cài | Phương án |
|---|---|
| Ubuntu/Debian VPS, máy ảo cloud x86_64/amd64 | Docker Compose theo tài liệu này |
| Linux muốn chạy trực tiếp/systemd | [Cài native Linux](INSTALL_NATIVE_VI.md) |
| Windows x64 | Docker Desktop, Linux amd64 containers, chạy Bash trong WSL2 Ubuntu |
| macOS Intel | Docker Desktop, Linux amd64 image |
| Apple Silicon/ARM64 hoặc NAS/VPS ARM | Native Linux ARM64 hoặc điều chỉnh Dockerfile theo mục 14 rồi tự build/smoke test; Dockerfile hiện tại chưa portable ARM |
| NAS x86_64 hỗ trợ Docker | Compose; thay mount/reverse proxy theo NAS |
| PaaS/Kubernetes/managed cloud | Image + PostgreSQL + volume + worker riêng, mục 14 |
| Hosting chỉ PHP/cPanel | Cần khả năng chạy binary/container, PostgreSQL và background worker; hosting chỉ PHP không đủ |

Lệnh dưới đây dùng **Bash**, không phải PowerShell/cmd. Runtime Docker Linux amd64
đã được kiểm tra trong cloud. Dockerfile hiện tại có đường dẫn thư viện
`/usr/lib/x86_64-linux-gnu` cố định, vì vậy hướng dẫn build Docker nguyên bản áp
dụng cho **amd64**. ARM64 cần sửa bước copy thư viện hoặc dùng native Linux và
build/smoke test tại máy đích; không dùng binary amd64 trực tiếp trên ARM64.

Mức khởi đầu tham khảo cho panel nhỏ: 2 vCPU, 2–4 GiB RAM, ít nhất 20 GiB disk.
Build release nên có 4 vCPU/8 GiB RAM và thêm 10–20 GiB trống cho image/cache.
Đây không phải benchmark. Dữ liệu mail nằm ở Stalwart; backup lớn cần thêm RAM/disk
hoặc pipeline snapshot khác. Có thể build trên máy mạnh rồi chuyển image.

## 2. Chuẩn bị domain và mạng

Ví dụ dùng `panel.example.com`, thư mục `/opt/ui-rust`, project Compose `ui-rust`.
Thay hostname bằng domain thật của bạn trong mọi lệnh. Trỏ DNS A về IP máy chủ;
nếu tạo AAAA thì IPv6 phải hoạt động. Security group/firewall cho phép 80/443 và
cổng SSH thực đang dùng. PostgreSQL không public; web chỉ bind loopback 8080.

Máy build cần registry Docker và crates.io. Khi bật chức năng tương ứng, panel cần
HTTPS tới Stalwart, `cloudflare-dns.com`, `userapi.sepay.vn` và
`userapi-sandbox.sepay.vn`; worker cần SMTP outbound 587. Cổng SMTP/IMAP inbound
của mail server được mở trên máy Stalwart riêng.

## 3. Cài Docker Engine và Compose

Nếu đã có Docker Engine và Compose v2 hoạt động, bỏ qua phần cài. Với máy mới
Ubuntu/Debian, đăng nhập bằng user có `sudo`:

```bash
sudo apt-get update
sudo apt-get install -y ca-certificates curl git python3 openssl nano
sudo install -m 0755 -d /etc/apt/keyrings
. /etc/os-release
case "$ID" in ubuntu|debian) ;; *) echo 'Dùng hướng dẫn Docker của distro'; exit 1 ;; esac
curl -fsSL "https://download.docker.com/linux/$ID/gpg" \
  | sudo tee /etc/apt/keyrings/docker.asc >/dev/null
sudo chmod a+r /etc/apt/keyrings/docker.asc
DOCKER_CODENAME="${UBUNTU_CODENAME:-$VERSION_CODENAME}"
cat <<EOF_APT | sudo tee /etc/apt/sources.list.d/docker.sources >/dev/null
Types: deb
URIs: https://download.docker.com/linux/$ID
Suites: $DOCKER_CODENAME
Components: stable
Architectures: $(dpkg --print-architecture)
Signed-By: /etc/apt/keyrings/docker.asc
EOF_APT
sudo apt-get update
sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
sudo systemctl enable --now docker
sudo docker version
sudo docker compose version
```

Dùng `docker compose` v2. Nếu máy đã có Docker/package khác, xử lý theo tài liệu
chính thức để giữ container/volume hiện hữu, không xóa dữ liệu cho một lần cài mới:
[Ubuntu](https://docs.docker.com/engine/install/ubuntu/),
[Debian](https://docs.docker.com/engine/install/debian/),
[Windows](https://docs.docker.com/desktop/setup/install/windows-install/),
[macOS](https://docs.docker.com/desktop/setup/install/mac-install/).
Trên Docker Desktop, bỏ `sudo` nếu Docker đã chạy bằng user hiện tại.

## 4. Lấy đúng bản nguồn

### GitHub: main hoặc commit đã rà soát

Đối với cài mới, clone vào `/opt/ui-rust` để các lệnh sau dùng cùng đường dẫn:

```bash
(
  set -e
  if [ -e /opt/ui-rust ] || [ -L /opt/ui-rust ]; then
    printf 'Đường dẫn cài đã có; chọn thư mục mới hoặc xử lý bản cũ trước.\n' >&2
    exit 1
  fi
  sudo install -d -o "$USER" -g "$(id -gn)" -m 0750 /opt/ui-rust
  git clone --branch main https://github.com/lehuunghi/UI-Rust.git /opt/ui-rust
  cd /opt/ui-rust
  git rev-parse HEAD
)
```

Ghi SHA vừa in vào hồ sơ triển khai. Để ghim một commit đã rà soát, thay
`COMMIT_SHA` bên dưới bằng SHA thật trước khi chạy:

```bash
cd /opt/ui-rust
git checkout --detach COMMIT_SHA && git rev-parse HEAD
```

Kiểm tra hướng dẫn trong chính commit đó và dùng cùng SHA khi build trên máy khác.
Nếu chọn thư mục checkout khác, thay `/opt/ui-rust` ở các bước sau. Không dùng
`git reset --hard` để xóa các thay đổi chưa lưu của bản đang triển khai.

### Gói snapshot nguồn, khi không dùng Git

Chép `ui-rust-source.tar.gz` và `SHA256SUMS` vào cùng thư mục trên máy đích,
ví dụ `~/ui-rust-download`. Tạo thư mục đó trước khi dùng SFTP/scp. Đây là phương
án thay thế bước clone, không giải nén thêm vào checkout đã có.

```bash
# Chạy trên máy giữ archive, thay user/hostname và port SSH nếu khác mặc định.
scp ui-rust-source.tar.gz SHA256SUMS user@server.example.com:~/ui-rust-download/
```

Trên máy đích:

```bash
(
  set -e
  cd ~/ui-rust-download
  sha256sum -c SHA256SUMS
  if [ -e /opt/ui-rust ] || [ -L /opt/ui-rust ]; then
    printf 'Đường dẫn cài đã có; chọn thư mục mới hoặc xử lý bản cũ trước.\n' >&2
    exit 1
  fi
  sudo install -d -o "$USER" -g "$(id -gn)" -m 0750 /opt/ui-rust
  tar -xzf ui-rust-source.tar.gz -C /opt/ui-rust
  cd /opt/ui-rust
  ls Cargo.toml Cargo.lock Dockerfile compose.yaml
)
```

Archive chứa `Cargo.toml` ngay tại root, không bọc thêm thư mục UI-Rust. Nhận cả
archive/checksum từ nguồn tin cậy. Gói không có `.env`, DB dump, secrets, build/cache.
Snapshot archive và image sẵn không tự cập nhật khi `main` thay đổi. Chọn đúng
commit/snapshot cho lần cài hoặc nâng cấp; không coi tên file cố định là bằng
chứng nó chứa HEAD mới nhất của GitHub.

## 5. Tạo .env cho cài mới

Chỉ sinh APP_KEY cho **cài mới**. Khi chuyển máy/restore, giữ APP_KEY của bản cũ.
Khóa này giải mã token, TOTP, job/notification và backup; phải giống nhau ở mọi
web/worker/CLI của cùng hệ thống. Sao lưu khóa riêng ngoài server.

Thay hostname ở dòng `PANEL_ORIGIN`:

```bash
cd /opt/ui-rust
umask 077
PANEL_ORIGIN=https://panel.example.com python3 - <<'PY'
from pathlib import Path
import os, secrets
p = Path('.env')
if p.exists():
    raise SystemExit('.env đã có; giữ nguyên và chỉnh có chủ đích.')
pw = secrets.token_hex(24)
key = secrets.token_hex(32)
p.write_text('\n'.join([
    'COMPOSE_PROJECT_NAME=ui-rust', 'POSTGRES_PASSWORD=' + pw,
    'DATABASE_URL=postgres://ui:' + pw + '@db:5432/ui',
    'APP_KEY=' + key, 'APP_URL=' + os.environ['PANEL_ORIGIN'].rstrip('/'),
    'BIND=0.0.0.0:8080', 'RUST_LOG=ui_rust=info,tower_http=info',
    'SMTP_HOST=', 'SMTP_USER=', 'SMTP_PASSWORD=', 'SMTP_FROM=',
    'SEPAY_API_KEY=', 'SEPAY_HMAC_SECRET=', 'SEPAY_ACCOUNT_NUMBER=',
    'SEPAY_BANK_CODE=', 'SEPAY_USER_API_TOKEN=', 'SEPAY_API_MODE=live',
    'SEPAY_POLL_SECONDS=0', ''
]))
p.chmod(0o600)
print('Đã tạo .env; không in khóa/mật khẩu.')
PY
nano .env
sudo docker compose --env-file .env config --quiet
```

APP_URL là origin ngoài `https://panel.example.com`, không có `/admin`, `/panel`
hoặc port nội bộ. Triển khai tại root của hostname riêng. HTTPS bật cookie Secure.
Giữ COMPOSE_PROJECT_NAME cố định để không vô tình tạo bộ volume khác khi đổi thư mục.

Compose lấy POSTGRES_PASSWORD để override DATABASE_URL của web/worker. Mật khẩu
hex ở trên tránh ký tự cần percent-encode. Nếu dùng DB ngoài, xem mục 14; chỉ sửa
DATABASE_URL trong `.env` sẽ chưa đổi được cấu hình Compose mặc định.
Biến đã export trong shell có thể ghi đè `.env`; đừng dùng shell chứa biến bản cài khác.
Đặt secret có `$`, `#`, khoảng trắng đúng cú pháp Compose dotenv theo [tài liệu cấu hình](CONFIG_SERVICES_VI.md).

Không commit/in `.env` hoặc chạy `docker compose config` không có `--quiet` vào log
công khai. Đổi POSTGRES_PASSWORD trong file không tự đổi password role trên volume cũ.

## 6. Build, migrate, chạy web và worker

Nếu dùng image amd64 được cung cấp ở mục 14, import image và thêm image override
trước bước này; bỏ dòng `docker compose build`, dùng `up --no-build`. Các bước tạo
DB, migrate và tạo admin vẫn cần thực hiện với cùng `.env`/APP_KEY.

```bash
cd /opt/ui-rust
sudo docker compose build
sudo docker compose up -d db
sudo docker compose exec db pg_isready -U ui -d ui
# Đợi DB trả accepting connections trước bước tiếp.
sudo docker compose run --rm --no-deps web migrate
sudo docker compose up -d web worker
sudo docker compose ps
curl --fail --silent --show-error http://127.0.0.1:8080/healthz
curl --fail --silent --show-error http://127.0.0.1:8080/readyz
curl --fail --silent --show-error http://127.0.0.1:8080/assets/app.js --output /dev/null
```

Nếu DB chưa sẵn sàng, xem log rồi chạy lại check, không xóa volume.
`healthz` chỉ kiểm tra web; `readyz` kiểm tra DB; kiểm tra `app.js` xác minh asset
đọc được với quyền runtime. Chưa chứng minh SMTP/mail/SePay hoạt động.
Migrations cũng tự chạy khi ứng dụng khởi tạo state; chưa có biến tắt migration.
Role app cần quyền schema/DDL và bảng `_sqlx_migrations`. Không sửa migration đã áp dụng.

Với cài mới chưa có tác vụ production, kiểm tra một vòng worker:

```bash
sudo docker compose run --rm --no-deps worker worker-once
sudo docker compose logs --tail=100 web worker
```

`worker-once` xử lý tác vụ thật. Worker liên tục là yêu cầu cho SMTP, sync, gia hạn,
monitoring và lịch backup. Không coi đây là lệnh health check chỉ đọc trên production.

## 7. Nginx và HTTPS

Nếu dùng Caddy/Traefik/load balancer/NAS proxy, cấu hình tương đương: hostname riêng,
TLS, proxy tới web:8080, body limit ≥16 MiB. Ví dụ Nginx trên host:

```bash
sudo apt-get install -y nginx certbot python3-certbot-nginx
sudo systemctl enable --now nginx
sudo nano /etc/nginx/sites-available/ui-rust
```

Nội dung, thay hostname:

```nginx
server {
    listen 80;
    server_name panel.example.com;
    client_max_body_size 16m;
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_connect_timeout 10s;
        proxy_read_timeout 180s;
        proxy_send_timeout 180s;
    }
}
```

```bash
sudo ln -s /etc/nginx/sites-available/ui-rust /etc/nginx/sites-enabled/ui-rust
sudo nginx -t
sudo systemctl reload nginx
sudo certbot --nginx -d panel.example.com --redirect \
  --email ops@example.com --agree-tos
sudo nginx -t
sudo systemctl reload nginx
sudo certbot renew --dry-run
curl --fail --silent --show-error https://panel.example.com/readyz
```

Thay domain/email thật; DNS và cổng 80 phải truy cập được từ Internet. Nếu symlink
đã có thì kiểm tra cấu hình hiện hữu. Chỉ đăng nhập sau khi HTTPS hoạt động.
Thêm rate limit theo IP tại proxy cho auth/trial công khai; ứng dụng không tin
X-Forwarded-For để xác định IP. Không dùng CORS `*` để lách lỗi CSRF/Origin.
Restore dài qua UI có thể vượt timeout; kiểm tra trạng thái trước retry hoặc dùng CLI.

## 8. Tạo admin lần đầu

Không có tài khoản/mật khẩu mặc định. Đoạn sau hỏi mật khẩu ẩn trong root Bash,
không đặt password literal vào history/command line:

```bash
cd /opt/ui-rust
sudo bash -c '
read -r -p "Email admin: " ADMIN_EMAIL
read -r -s -p "Mật khẩu admin (ít nhất 12 ký tự): " ADMIN_PASSWORD
printf "\n"
export ADMIN_EMAIL ADMIN_PASSWORD
docker compose run --rm --no-deps -e ADMIN_EMAIL -e ADMIN_PASSWORD web create-admin
result=$?
unset ADMIN_EMAIL ADMIN_PASSWORD
exit "$result"
'
```

Trên Desktop không cần sudo: dùng `bash -c` với cùng nội dung. Chạy một lần; email
trùng bị từ chối. Mở `https://panel.example.com/dang-nhap`, bật TOTP trong bảo mật.
Đồng bộ giờ. Cấu hình/gửi thử SMTP trước khi chọn email OTP.

## 9. Thiết lập chức năng

Theo [CONFIG_SERVICES_VI.md](CONFIG_SERVICES_VI.md) để cấu hình chi tiết:

1. Thương hiệu, nội dung trang chủ, ngôn ngữ, mẫu email.
2. Gói: giá, số mailbox/domain, dung lượng, quyền alias/admin phụ.
3. SMTP STARTTLS **587**; recreate web/worker sau sửa env, gửi thử reset tới email của bạn.
4. Stalwart HTTPS/token/quyền đúng phiên bản; chọn primary/placement. Dry-run dùng
   DB thử riêng, không chuyển tài nguyên `dry-*` trực tiếp sang live.
5. Khách → domain → TXT ownership → nhóm → hộp thư. Xem job synced, kiểm tra login,
   send/receive mail thật; cấu hình MX/SPF/DKIM/DMARC/PTR/TLS riêng của mail server.
6. SePay: tài khoản/bank/webhook auth, sandbox/API và ledger live đúng chế độ.
7. Lịch backup, APP_KEY ngoài máy và restore thử trên DB/môi trường cô lập.

Xem [PARITY.md](PARITY.md) để biết phần đã triển khai, giới hạn và phần chưa tương đương PHP.

## 10. Cloud backup/mailbox/recovery tùy chọn

Chỉ `db`, `web`, `worker` cần cho panel cơ bản. Trước khi bật profile,
chuẩn bị Vandelay, Stalwart CLI, rclone config, nguồn snapshot và đích cô lập theo
[CONFIG_SERVICES_VI.md](CONFIG_SERVICES_VI.md). Không dùng CLI fixture của test để vận hành.

Nếu bạn tạo `compose.integrations.yaml` và `compose.mail-tools.yaml` theo tài liệu
cấu hình, giữ các overlay trong **mọi** lệnh Compose của bản cài này. Có thể thêm
dòng sau vào `.env` để các lệnh không có `-f` tự dùng đủ file:

```dotenv
COMPOSE_FILE=compose.yaml:compose.integrations.yaml:compose.mail-tools.yaml
```

Chỉ liệt kê file đã tạo. Với WSL2/Linux/macOS, dấu phân cách là `:`. Khi đặt
COMPOSE_FILE, Compose không tự thêm `compose.override.yaml`; nếu dùng file đó,
thêm nó vào danh sách. Khi chạy lệnh có `-f` rõ ràng (như restore ở mục 11), thêm
các overlay cần giữ bằng `-f`; đừng bỏ mount backup/config của bản cài hiện tại.

```bash
sudo docker compose --profile mail-recovery up -d mailbox-worker recovery-worker
sudo docker compose --profile mail-recovery ps
sudo docker compose logs --tail=100 mailbox-worker recovery-worker
```

Image mặc định có OpenSSL, rclone, PostgreSQL client 17; chưa có Vandelay/Stalwart CLI.
Ứng dụng Docker UID **10001** cần đọc executable/config và ghi backup/staging.
Mọi service phải dùng cùng APP_KEY và mount path/volume. Backup panel không chứa
mail/blob/backend Stalwart. Archive mailbox/recovery tối đa 256 MiB; phục hồi backend
đầy đủ phải diễn tập và kiểm chứng mail/thư mục/attachment/queue thật.

## 11. Backup và diễn tập restore panel

Sao lưu:

```bash
sudo docker compose run --rm --no-deps web backup
sudo docker compose cp web:/app/backups ./backup-export
sudo chown -R "$USER:$(id -gn)" ./backup-export
chmod -R go-rwx ./backup-export
```

CLI ghi `backups/ui-<UTC>-<random>.pgenc`; UI/worker dùng namespace bên dưới backups.
Backup CLI không tự xuất hiện trong danh sách UI. Lưu `.pgenc` và APP_KEY tách biệt,
ngoài máy. Kiểm tra bằng restore thật, không chỉ nhìn dung lượng file.

### Restore thử, không thay thế DB đang phục vụ

```bash
sudo docker compose exec db createdb -U ui -O ui ui_restore_test
```

Tạo env restore riêng; script chỉ dùng cho `.env` được sinh tại mục 5 (hex đơn giản).
Với cấu hình khác, đặt URL đích được percent-encode trong file riêng bằng editor.

```bash
umask 077
python3 - <<'PY'
from pathlib import Path
import re
v = dict(line.split('=', 1) for line in Path('.env').read_text().splitlines()
         if line and not line.startswith('#') and '=' in line)
if not re.fullmatch(r'[0-9a-f]{48}', v['POSTGRES_PASSWORD']):
    raise SystemExit('Dùng editor đặt URL đích phù hợp cho mật khẩu đã chỉnh.')
p = Path('.restore-test.env')
if p.exists():
    raise SystemExit('File env restore đã có; giữ nguyên để kiểm tra.')
p.write_text('RESTORE_DATABASE_URL=postgres://ui:' + v['POSTGRES_PASSWORD'] +
             '@db:5432/ui_restore_test\n')
p.chmod(0o600)
PY
cat > compose.restore-test.yaml <<'YAML'
services:
  web:
    environment:
      DATABASE_URL: ${RESTORE_DATABASE_URL:?Set isolated restore URL}
YAML
sudo docker compose --env-file .env --env-file .restore-test.env \
  -f compose.yaml -f compose.restore-test.yaml config --quiet
# Thay đường dẫn bằng .pgenc thật đang có trong volume.
sudo docker compose --env-file .env --env-file .restore-test.env \
  -f compose.yaml -f compose.restore-test.yaml run --rm --no-deps web \
  restore /app/backups/ui-YYYYMMDDTHHMMSSZ-RANDOM.pgenc --confirm-db ui_restore_test
sudo docker compose exec db psql -U ui -d ui_restore_test -c \
  'SELECT count(*) AS users FROM users; SELECT count(*) AS subscriptions FROM subscriptions; SELECT count(*) AS invoices FROM subscriptions WHERE invoice_code IS NOT NULL; SELECT count(*) AS payments FROM payment_transactions;'
rm .restore-test.env compose.restore-test.yaml
```

Không chạy `up` với override restore. CLI có `--clean` và chỉ kiểm tra tên DB,
không tự bảo vệ DB production như UI; xác nhận đúng DB cô lập. Kiểm tra ledger,
row counts, mapping và giải mã. Role restore cần đủ quyền, pg_restore tương thích PG17.
Nếu có overlay riêng thay mount backup/volume hoặc kết nối DB, thêm overlay đó
trước `-f compose.restore-test.yaml` ở các lệnh trên; override restore phải là file
cuối để DATABASE_URL trỏ đúng DB thử. Không khởi động web/worker trên DB thử đã
sao chép production khi còn credential/job thật.

## 12. Nâng cấp và rollback

Backup DB/volume/APP_KEY, ghi commit/checksum/image, thử bản mới trên DB sao chép
cô lập trước. Giữ `.env`/volume khi thay source. Build trước khi dừng bản đang chạy:

```bash
sudo docker compose --profile mail-recovery build &&
sudo docker compose --profile mail-recovery stop web worker mailbox-worker recovery-worker &&
sudo docker compose run --rm --no-deps web migrate &&
sudo docker compose up -d web worker &&
curl --fail --silent --show-error http://127.0.0.1:8080/readyz &&
sudo docker compose logs --tail=100 web worker
```

Nếu trước đó đã bật optional workers, sau khi kiểm tra migration/web thành công:

```bash
# Chỉ start các worker mail mà bản cài này đã cấu hình/sử dụng.
sudo docker compose --profile mail-recovery up -d mailbox-worker recovery-worker
```

Build với profile giúp mọi image web/worker mail cùng bản mới; dừng tất cả trước
migration để tránh worker cũ xử lý schema mới. Không chạy tiếp lệnh `up` nếu lệnh
migrate thất bại. Overlay tool/rclone phải tiếp tục được dùng khi nâng cấp.

Nếu migration thất bại, đọc log và giữ hệ thống dừng để xử lý đúng nguyên nhân.
Không sinh lại APP_KEY/xóa volume. Migration mới có thể không tương thích binary cũ;
rollback cần kế hoạch schema hoặc restore đầy đủ, không chỉ đổi image.
Job uncertain/unconfirmed cần kiểm tra remote trước retry để tránh tạo/ghi trùng.

## 13. Di chuyển sang máy khác

1. Chuẩn bị máy mới với đúng phiên bản nguồn/image.
2. Dừng web và **mọi worker** máy cũ, quản lý webhook/proxy trong cửa sổ bảo trì;
   giữ PG chạy để tạo backup cuối bằng CLI.
3. Copy `.pgenc`, archive volume cần giữ, `.env`/APP_KEY qua kênh an toàn; chuyển
   cả rclone/CLI/adapter/snapshot nếu dùng các chức năng đó.
4. Máy mới tạo DB/role rỗng, restore CLI vào DB đích `ui` **trước** khi khởi động app.
   Giữ APP_KEY cũ; sửa APP_URL/DB credentials có chủ đích. Không auto migrate DB rỗng
   trước restore nếu định restore toàn bộ schema cũ.
5. Giữ/rehome namespace backup trước khi bật ứng dụng nếu host/port/dbname thay đổi
   theo [quy trình archive](CONFIG_SERVICES_VI.md#giữ-archive-khi-thay-địa-chỉ-database-hoặc-chuyển-nơi-cài).
   Chạy migrate bằng phiên bản đúng, kiểm tra dữ liệu/giải mã/readyz, chuyển DNS/proxy,
   bật web/worker máy mới và kiểm tra mail/webhook.
6. Giữ máy cũ dừng; hai DB sao chép với hai worker đồng thời sẽ xử lý tác vụ trùng.
   Giữ bản sao cũ đủ lâu để xác minh rồi thu hồi cấu hình không còn dùng.

Đổi hostname/port/database trong DATABASE_URL sẽ đổi namespace archive UI.
Xem quy trình rehome tại CONFIG_SERVICES_VI.md. Restore panel không tự di chuyển
backend/thư của Stalwart và không phải công cụ nhập toàn bộ MySQL/PHP.

## 14. Cài ở nhiều nơi và PaaS

### Dùng image amd64 được cung cấp

Gói `ui-rust-image-amd64.tar.gz` kèm `ui-rust-image-amd64.tar.gz.sha256` chứa runtime
image đã kiểm chứng trên Linux **amd64**, tag `ui-rust:2026-10-08-amd64`. Không dùng
trực tiếp trên ARM64. Nhận image/checksum cùng nguồn tin cậy; đây là checksum
riêng, không nằm trong file `SHA256SUMS` của gói source.
Image này là snapshot của đợt kiểm chứng, không tự theo HEAD của `main`. Khi cần
commit khác, build image từ commit đó hoặc dùng image có bằng chứng tương ứng.

```bash
# Chạy tại thư mục chứa hai file image/checksum.
sha256sum -c ui-rust-image-amd64.tar.gz.sha256 &&
gunzip -c ui-rust-image-amd64.tar.gz | sudo docker load
```

Trong thư mục source của bản cài (`/opt/ui-rust`), thêm cấu hình dưới đây vào
`compose.override.yaml`. Nếu file đã tồn tại, merge theo service; không ghi đè
mount/port/tool config của bạn:

```yaml
services:
  web:
    image: ui-rust:2026-10-08-amd64
  worker:
    image: ui-rust:2026-10-08-amd64
  mailbox-worker:
    image: ui-rust:2026-10-08-amd64
  recovery-worker:
    image: ui-rust:2026-10-08-amd64
```

Nếu đặt `COMPOSE_FILE`, phải thêm `compose.override.yaml` vào danh sách hoặc thêm
`-f` tương ứng. Tạo `.env` theo mục 5, rồi dùng quy trình dưới đây thay phần build
ở mục 6; không cần chạy `docker compose build` trên máy đích:

```bash
cd /opt/ui-rust
sudo docker compose config --quiet &&
sudo docker compose up -d --no-build db
sudo docker compose exec db pg_isready -U ui -d ui
# Đợi DB trả accepting connections trước bước tiếp.
sudo docker compose run --rm --no-deps web migrate &&
sudo docker compose up -d --no-build web worker
curl --fail --silent --show-error http://127.0.0.1:8080/readyz
curl --fail --silent --show-error --output /dev/null http://127.0.0.1:8080/assets/app.js
```

Image không chứa `.env`, DB hay volume backup. Optional workers chỉ bật khi đã
cấu hình tool/mount đúng theo mục 10. Khi nâng phiên bản image sẵn, import image
mới, đổi tag **mọi service**, dừng workers/migrate và restart; bỏ bước build source
trong mục 12. Giữ image cũ và backup để rollback có kiểm chứng schema.

### Tự build rồi chuyển image

Build image trên máy đủ RAM, cùng kiến trúc máy đích. Các lệnh Dockerfile nguyên
bản bên dưới dành cho **amd64**:

```bash
sudo docker build -t ui-rust:my-release .
sudo docker save ui-rust:my-release | gzip > ui-rust-image.tar.gz
sha256sum ui-rust-image.tar.gz > ui-rust-image.tar.gz.sha256
# Chép image và checksum sang máy đích.
sha256sum -c ui-rust-image.tar.gz.sha256
gunzip -c ui-rust-image.tar.gz | sudo docker load
```

Ở máy đích, thêm `compose.override.yaml`, rồi `up --no-build`:

```yaml
services:
  web:
    image: ui-rust:my-release
  worker:
    image: ui-rust:my-release
  mailbox-worker:
    image: ui-rust:my-release
  recovery-worker:
    image: ui-rust:my-release
```

```bash
sudo docker compose up -d --no-build db web worker
```

Image không chứa secrets/DB/backup; vẫn cần .env và volumes. Nếu cài nhiều panel
trên cùng host, dùng project/domain/env riêng. Bản thứ hai đổi host port bằng override
Compose ≥2.24.4, proxy của domain thứ hai tới 8081:

```yaml
services:
  web:
    ports: !override
      - '127.0.0.1:8081:8080'
```

Giữ BIND container `0.0.0.0:8080`. Trên PaaS dùng command web `serve` và background
`worker`, PostgreSQL, persistent backup/staging, APP_URL ngoài. App đọc BIND, chưa
tự đọc PORT; đặt BIND theo port nền tảng yêu cầu. PaaS chỉ web không đủ workflow.

Managed PostgreSQL cần deployment file riêng/override loại bỏ DB local/dependency
và DATABASE_URL hardcoded trong Compose. Không chỉ sửa `.env` để đổi host.
Kiểm tra schema/DDL, PG17 client backup, SSL/CA/network theo nhà cung cấp. Có URL
web kết nối được chưa chắc pg_dump cũng có đủ quyền/trust.

### Nếu máy đích là ARM64

Dockerfile hiện copy libpq từ đường dẫn x86_64 cố định; đổi `--platform` một mình
không sửa được bước đó. Chọn [native Linux](INSTALL_NATIVE_VI.md) để build trên
máy ARM64, hoặc tạo Dockerfile riêng, sửa stage pgtools để gom thư viện từ thư mục
kiến trúc thực rồi copy thư mục gom vào runtime. Ví dụ thay hai phần tương ứng:

```dockerfile
FROM postgres:17-bookworm AS pgtools
RUN mkdir -p /pg-libs && cp -a /usr/lib/*-linux-gnu/libpq.so.5* /pg-libs/

# Trong runtime stage, thay COPY libpq hardcoded bằng:
COPY --from=pgtools /pg-libs/ /usr/local/lib/
```

Giữ bước copy `pg_dump`/`pg_restore` và `RUN ldconfig` của Dockerfile. Đây là hướng
điều chỉnh chưa được kiểm chứng ARM trong cloud này, không phải image ARM đã phát
hành. Build trên host ARM hoặc buildx có runtime/emulation phù hợp, kiểm tra
`ui-rust keygen`, web/worker/readyz và backup/restore PG thật trước triển khai. Tool
Vandelay/Stalwart CLI cũng phải đúng ARM64 và loader/thư viện runtime.

## 15. Vận hành và xử lý lỗi

```bash
sudo docker compose ps
sudo docker compose logs --since=10m --tail=200 web worker
curl --fail --silent --show-error http://127.0.0.1:8080/readyz
df -h
# Sau khi chỉnh .env:
sudo docker compose up -d --force-recreate web worker
```

Theo dõi worker/job/SMTP, backup ngoài máy, disk/RAM, TLS renewal, mail và payment.
`docker compose down` giữ named volumes; **`down -v` xóa dữ liệu volumes**, không dùng
để restart/cập nhật. Running container chưa chứng minh dịch vụ nghiệp vụ hoạt động.

| Lỗi | Cách kiểm tra |
|---|---|
| Build thiếu RAM/exit137, disk đầy | Build trên máy đủ RAM/disk; chỉ dọn cache tái tạo được, giữ source/DB/volume |
| APP_KEY sai | Đúng 64 hex, không khoảng trắng; dùng khóa cũ khi restore |
| DB password fail | Role/password thật, host; đổi env không đổi role của volume cũ |
| Web restart/migration fail | Xem logs web/db và quyền schema, không xóa volume để lách lỗi |
| healthz OK nhưng readyz lỗi | DB/network/credential; healthz không kiểm tra PostgreSQL |
| 403 Origin/CSRF | APP_URL khớp scheme/hostname/port trình duyệt, HTTPS, đăng nhập lại; không tắt CSRF |
| 413 upload | Body limit proxy và route, trial tối đa5MiB/tệp |
| CSS/JS404 native | WorkingDirectory/static đúng bản release, quyền đọc |
| Mailbox pending | Worker, token/quyền/URL Stalwart, job error/version; đọc remote trước retry |
| Không có OTP/reset/trial welcome | SMTP STARTTLS587, FROM/app password, template enabled, worker; 465 chưa cấu hình được |
| TOTP sai | Giờ server/điện thoại, replay code đã dùng |
| Backup permission fail | UID10001, shared volume/mount path, executable/config readable |
| Cloud fail | rclone config/remote/network/quota, checksum/APP_KEY; không sửa checksum để ép restore |
| Certificate/CA fail | Hostname/full chain/trustedCA; không tắt TLS/checksum/package signature |

### Build sau TLS proxy của tổ chức

Cloud hiện tại cần CA host cho Cargo trong container. Khi kiểm chứng image, dùng
build secret chứa trust bundle; không đưa CA nội bộ đó vào gói nguồn hoặc máy khác.
Nếu máy bạn có proxy TLS, chỉ dùng CA được quản trị mạng xác nhận. Tạo
`Dockerfile.build-ca` từ Dockerfile gốc, đổi dòng Cargo thành:

```dockerfile
RUN --mount=type=secret,id=build_ca,target=/run/secrets/build-ca.pem \
    CARGO_HTTP_CAINFO=/run/secrets/build-ca.pem cargo build --release --locked -j4
```

```bash
sudo docker build --secret id=build_ca,src=/path/to/trusted-ca-bundle.pem \
  -f Dockerfile.build-ca -t ui-rust:my-release .
```

Các bước apt/network khác cũng cần trust phù hợp hạ tầng. Không `curl -k`, không
tắt TLS/chữ ký/checksum để làm build qua. Với mạng bình thường dùng Dockerfile gốc.

## 16. Checklist hoàn tất

- [ ] Đúng source/commit/kiến trúc, build qua, .env/APP_KEY bảo vệ và sao lưu.
- [ ] PostgreSQL persistent, migration qua; web/worker restart được.
- [ ] HTTPS/readyz/assets/renewal qua, port nội bộ không public.
- [ ] Admin tạo một lần, TOTP và SMTP gửi nhận thật nếu dùng.
- [ ] Domain/mailbox synced, login/send/receive qua Stalwart thật.
- [ ] SePay thật/sandbox đúng chế độ nếu dùng.
- [ ] Backup ngoài máy, restore DB cô lập qua; optional recovery kiểm chứng riêng.
- [ ] Ghi phiên bản, mount paths, nâng cấp/rollback và giới hạn đã biết.

Đọc tiếp: [native Linux](INSTALL_NATIVE_VI.md), [cấu hình dịch vụ](CONFIG_SERVICES_VI.md),
[triển khai](DEPLOYMENT.md), [đối chiếu](PARITY.md), [migration dữ liệu](MIGRATION.md).
