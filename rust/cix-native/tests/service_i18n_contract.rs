//! Focused HOST contract using actual maintained modules, without the service engine.
//! Requires server dependencies and Local's regenerated English catalog.
#![cfg(feature = "server")]

#[path = "../src/i18n.rs"]
mod i18n;
#[path = "../src/service/error.rs"]
pub mod service_error;
mod service {
    pub use crate::service_error as error;
}
#[path = "../src/service/problem_message.rs"]
mod presentation;

use i18n::{Localizer, Value};
use presentation::{problem_message, PresentationError};
use service::error::Problem;
use serde_json::json;

#[test]
fn core_constructors_render_english_and_preserve_machine_envelope() {
    let examples = [
        (Problem::invalid("name"), "Invalid request field: name.", 400, false),
        (Problem::limit("bytes"), "Resource limit exceeded: bytes.", 413, false),
        (Problem::missing(), "Resource not found.", 404, false),
        (Problem::conflict(), "Revision conflict.", 409, false),
        (Problem::internal(), "Internal error.", 500, false),
        (Problem::from(std::io::Error::from(std::io::ErrorKind::Other)), "Storage is unavailable.", 503, true),
        (Problem::from(sqlx::Error::RowNotFound), "Metadata is unavailable.", 503, true),
    ];
    for (problem, expected, status, retryable) in examples {
        let before = serde_json::to_vec(&problem).unwrap();
        let display = problem.to_string();
        let message = problem_message(&problem).unwrap();
        assert_eq!(message.id, problem.message_id);
        assert_eq!(Localizer::english().render(&message).unwrap(), expected);
        assert_eq!((problem.status, problem.retryable), (status, retryable));
        assert_eq!(problem.to_string(), display);
        assert_eq!(serde_json::to_vec(&problem).unwrap(), before);
    }
}

#[test]
fn placeholder_looking_identifiers_are_literal_code_values() {
    let literal = "{field} {{resource}} } {service.error.internal}";
    for (problem, key, prefix) in [
        (Problem::invalid(literal), "field", "Invalid request field: "),
        (Problem::limit(literal), "resource", "Resource limit exceeded: "),
    ] {
        let before = serde_json::to_vec(&problem).unwrap();
        let message = problem_message(&problem).unwrap();
        assert_eq!(message.args[key], Value::Code(literal.to_owned()));
        assert_eq!(Localizer::english().render(&message).unwrap(), format!("{prefix}{literal}."));
        assert_eq!(serde_json::to_vec(&problem).unwrap(), before);
    }
}

fn rejects_unchanged(problem: Problem, expected: PresentationError) {
    let before = serde_json::to_vec(&problem).unwrap();
    let status = problem.status;
    let retryable = problem.retryable;
    let display = problem.to_string();
    assert_eq!(problem_message(&problem), Err(expected));
    assert_eq!(serde_json::to_vec(&problem).unwrap(), before);
    assert_eq!((problem.status, problem.retryable), (status, retryable));
    assert_eq!(problem.to_string(), display);
}

#[test]
fn unsupported_codes_and_mismatched_ids_reject_unchanged() {
    rejects_unchanged(Problem::new("forbidden", 403), PresentationError::UnknownCode);
    let mut problem = Problem::invalid("name");
    problem.message_id = "service.error.internal".into();
    rejects_unchanged(problem, PresentationError::MessageIdMismatch);
}

#[test]
fn argument_shape_missing_extra_and_types_reject_unchanged() {
    for code in ["invalid_argument", "resource_limit", "not_found", "revision_conflict", "internal", "storage_io", "metadata_unavailable"] {
        for arguments in [json!(null), json!([]), json!("field"), json!(42), json!(true)] {
            let mut problem = Problem::new(code, 503);
            problem.arguments = arguments;
            rejects_unchanged(problem, PresentationError::ArgumentsNotObject);
        }
        let mut extra = match code {
            "invalid_argument" => Problem::invalid("name"),
            "resource_limit" => Problem::limit("bytes"),
            _ => Problem::new(code, 503),
        };
        extra.arguments["extra"] = json!("ignored?");
        rejects_unchanged(extra, PresentationError::UnexpectedArgument("extra".into()));
    }
    for (code, name) in [("invalid_argument", "field"), ("resource_limit", "resource")] {
        rejects_unchanged(Problem::new(code, 400), PresentationError::MissingArgument(name));
        for value in [json!(null), json!(true), json!(1), json!([]), json!({})] {
            let mut problem = Problem::new(code, 400);
            problem.arguments[name] = value;
            rejects_unchanged(problem, PresentationError::InvalidArgumentType(name));
        }
    }
}
