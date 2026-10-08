use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
#[derive(Debug)]
pub struct Error(pub StatusCode, pub String);
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn bad(s: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, s.into())
    }
    pub fn forbidden() -> Self {
        Self(StatusCode::FORBIDDEN, "Không có quyền truy cập".into())
    }
    pub fn missing() -> Self {
        Self(StatusCode::NOT_FOUND, "Không tìm thấy dữ liệu".into())
    }
    pub fn unauthorized() -> Self {
        Self(StatusCode::UNAUTHORIZED, "Vui lòng đăng nhập".into())
    }
    pub fn conflict(s: impl Into<String>) -> Self {
        Self(StatusCode::CONFLICT, s.into())
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        if let sqlx::Error::Database(d) = &e {
            if d.is_unique_violation() {
                return Self::conflict("Dữ liệu đã tồn tại");
            };
            if d.is_foreign_key_violation() {
                return Self::bad("Dữ liệu đang được tham chiếu hoặc không tồn tại");
            };
            if d.is_check_violation() {
                return Self::bad("Giá trị không hợp lệ");
            }
        }
        tracing::error!(error=%e,"database operation failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Lỗi cơ sở dữ liệu".into(),
        )
    }
}
impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!(error=%e,"operation failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Không thể hoàn thành thao tác".into(),
        )
    }
}
