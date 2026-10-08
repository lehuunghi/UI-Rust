INSERT INTO email_templates(template_key,name,subject,body,enabled) VALUES
('password_reset','Đặt lại mật khẩu','Đặt lại mật khẩu {{site_name}}','Xin chào {{name}},

Mở liên kết để đặt lại mật khẩu: {{reset_url}}
Liên kết có hiệu lực 30 phút và chỉ dùng một lần. Nếu bạn không yêu cầu, bỏ qua email này.',1),
('customer_registered','Đăng ký tài khoản khách hàng','Tài khoản {{site_name}} của bạn đã được tạo','Xin chào {{name}},

Tài khoản của bạn đã được tạo trên {{site_name}}.
Email đăng nhập: {{email}}
{{password_line}}
Đăng nhập: {{login_url}}

Trân trọng.',1),
('invoice_created','Tạo hóa đơn mua gói','Hóa đơn {{invoice_code}} đã được tạo','Xin chào {{name}},

Hóa đơn {{invoice_code}} cho gói {{package_name}} đã được tạo.
Tổng tiền: {{amount}}
Thanh toán QR: {{invoice_url}}

Sau khi SePay xác nhận thanh toán, gói sẽ tự kích hoạt.',1),
('subscription_activated','Kích hoạt gói','Gói {{package_name}} đã được kích hoạt','Xin chào {{name}},

Gói {{package_name}} đã được kích hoạt.
Email tối đa: {{email_limit}}
Domain tối đa: {{domain_limit}}
Hết hạn: {{end_date}}

Bạn có thể đăng nhập để quản lý domain và email.',1),
('email_account_created','Tạo tài khoản email','Tài khoản email {{email}} đã được tạo','Xin chào {{name}},

Tài khoản email đã được tạo.
Email: {{email}}
Mật khẩu: {{password}}
Nhóm ID: {{group_id}}

Vui lòng đổi mật khẩu sau khi đăng nhập lần đầu.',1),
('login_otp','Mã xác thực 2FA khi đăng nhập','Mã xác thực đăng nhập {{site_name}}','Xin chào {{name}},

Mã xác thực đăng nhập của bạn là: {{code}}
Mã có hiệu lực trong {{expires_minutes}} phút.
IP đăng nhập: {{ip}}

Nếu bạn không thực hiện đăng nhập này, hãy đổi mật khẩu ngay.',1),
('login_success','Thông báo đăng nhập thành công','Tài khoản {{site_name}} vừa đăng nhập thành công','Xin chào {{name}},

Tài khoản {{email}} vừa đăng nhập thành công.
Thời gian: {{login_time}}
IP: {{ip}}
Thiết bị: {{user_agent}}

Nếu đây không phải bạn, vui lòng đổi mật khẩu và liên hệ quản trị.',1),
('trial_request_created','Nhận đăng ký dùng thử','Yêu cầu dùng thử {{package_name}} đã được ghi nhận','Xin chào {{name}},

Chúng tôi đã nhận yêu cầu dùng thử gói {{package_name}} trong 14 ngày.
Admin sẽ kiểm tra thông tin và kích hoạt nếu hồ sơ hợp lệ.

Trân trọng.',1),
('trial_activated','Kích hoạt dùng thử','Tài khoản dùng thử {{package_name}} đã được kích hoạt','Xin chào {{name}},

Tài khoản dùng thử gói {{package_name}} đã được kích hoạt đến {{end_date}}.
Email đăng nhập: {{email}}
{{password_line}}
Đăng nhập: {{login_url}}

Bạn có thể nâng cấp lên trả phí bất cứ lúc nào.',1),
('renewal_reminder','Nhắc gia hạn gói','Gói {{package_name}} còn {{days_left}} ngày sử dụng','Xin chào {{name}},

Gói {{package_name}} của bạn sẽ hết hạn vào {{end_date}}.
Hệ thống đã tạo hóa đơn gia hạn tại: {{renewal_url}}

Nếu quá hạn 15 ngày, tài khoản và email/domain sẽ bị ngừng hoạt động.',1),
('notice','Thông báo chung','{{subject}}','{{message}}

{{site_name}}',1)
ON CONFLICT DO NOTHING;
