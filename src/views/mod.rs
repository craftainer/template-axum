//! The View layer: DTOs (de)serialized at the HTTP boundary, translated
//! from the ORM model. Sits above `models` in the layering order (see
//! `src/README.md`) -- may import `models` types for `From` conversions,
//! never the reverse.

pub mod bulk;
pub mod hero;
pub mod hero_xml;

/// One field-level validation failure -- rendered into RFC 9457's `detail`
/// as a list, mirroring FastAPI/Pydantic's `RequestValidationError.errors()`
/// shape (`loc`/`msg`) closely enough for a client to key off `field`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FieldError {
    pub field: &'static str,
    pub msg: String,
}

impl FieldError {
    pub fn new(field: &'static str, msg: impl Into<String>) -> Self {
        Self {
            field,
            msg: msg.into(),
        }
    }
}
