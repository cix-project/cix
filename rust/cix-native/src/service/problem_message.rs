//! Typed human presentation for the seven core service problems.
use crate::{i18n::{Message, Value}, service::error::Problem};

/// A malformed or unsupported presentation envelope. Machine data stays intact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresentationError {
    UnknownCode,
    MessageIdMismatch,
    ArgumentsNotObject,
    MissingArgument(&'static str),
    UnexpectedArgument(String),
    InvalidArgumentType(&'static str),
}

impl std::fmt::Display for PresentationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCode => f.write_str("unsupported service problem code"),
            Self::MessageIdMismatch => f.write_str("service problem message ID does not match code"),
            Self::ArgumentsNotObject => f.write_str("service problem arguments must be an object"),
            Self::MissingArgument(name) => write!(f, "missing presentation argument: {name}"),
            Self::UnexpectedArgument(name) => write!(f, "unexpected presentation argument: {name}"),
            Self::InvalidArgumentType(name) => write!(f, "presentation argument must be a string: {name}"),
        }
    }
}
impl std::error::Error for PresentationError {}

/// Convert without changing code, arguments, wire fields, status or retryability.
/// Codes outside this bounded core set are explicitly unsupported.
pub fn problem_message(problem: &Problem) -> Result<Message, PresentationError> {
    let (id, parameter) = match problem.code.as_str() {
        "invalid_argument" => ("service.error.invalid_argument", Some("field")),
        "resource_limit" => ("service.error.resource_limit", Some("resource")),
        "not_found" => ("service.error.not_found", None),
        "revision_conflict" => ("service.error.revision_conflict", None),
        "internal" => ("service.error.internal", None),
        "storage_io" => ("service.error.storage_io", None),
        "metadata_unavailable" => ("service.error.metadata_unavailable", None),
        _ => return Err(PresentationError::UnknownCode),
    };
    if problem.message_id != id {
        return Err(PresentationError::MessageIdMismatch);
    }
    let arguments = problem.arguments.as_object()
        .ok_or(PresentationError::ArgumentsNotObject)?;
    if let Some(name) = parameter {
        if !arguments.contains_key(name) {
            return Err(PresentationError::MissingArgument(name));
        }
    }
    for name in arguments.keys() {
        if Some(name.as_str()) != parameter {
            return Err(PresentationError::UnexpectedArgument(name.clone()));
        }
    }
    let message = Message::new(id);
    match parameter {
        Some(name) => {
            let value = arguments[name].as_str()
                .ok_or(PresentationError::InvalidArgumentType(name))?;
            Ok(message.arg(name, Value::Code(value.to_owned())))
        }
        None => Ok(message),
    }
}
