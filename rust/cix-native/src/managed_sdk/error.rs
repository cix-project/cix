//! Stable errors, independent of transport status or translated presentation.
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidConfiguration(&'static str),
    InvalidRequest(&'static str),
    PermissionDenied,
    SessionExpired,
    ResourceLimit(&'static str),
    Internal,
}

impl Error {
    /// Typed human presentation using the existing immutable localization context.
    /// Stable machine codes, message IDs and Display remain unchanged.
    pub fn message(self) -> crate::i18n::Message {
        use crate::i18n::{Message, Value};
        let message = Message::new(self.message_id());
        match self {
            Self::InvalidConfiguration(field) | Self::InvalidRequest(field) => {
                message.arg("field", Value::Code(field.into()))
            }
            Self::ResourceLimit(resource) => {
                message.arg("resource", Value::Code(resource.into()))
            }
            Self::PermissionDenied | Self::SessionExpired | Self::Internal => message,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidConfiguration(_) => "invalid_configuration",
            Self::InvalidRequest(_) => "invalid_argument",
            Self::PermissionDenied => "forbidden",
            Self::SessionExpired => "session_expired",
            Self::ResourceLimit(_) => "resource_limit",
            Self::Internal => "internal",
        }
    }

    pub fn message_id(self) -> &'static str {
        match self {
            Self::InvalidConfiguration(_) => "managed.error.invalid_configuration",
            Self::InvalidRequest(_) => "managed.error.invalid_argument",
            Self::PermissionDenied => "managed.error.forbidden",
            Self::SessionExpired => "managed.error.session_expired",
            Self::ResourceLimit(_) => "managed.error.resource_limit",
            Self::Internal => "managed.error.internal",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for Error {}
