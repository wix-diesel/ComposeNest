//! Application use cases and their dependency ports.

use composenest_domain::application_title;
use serde::{Deserialize, Serialize};

pub mod host_ports;
pub mod operation_journal;
pub mod state_store;
pub mod storage;
pub mod template_catalog;

/// The first version of the desktop IPC contract.
pub const API_VERSION: u16 = 1;

/// Provides the current Unix time to application use cases.
pub trait Clock {
    /// Returns the current Unix time in whole seconds.
    fn unix_seconds(&self) -> u64;
}

/// Provides the data required by the initial desktop screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bootstrap {
    /// The product name to display.
    pub application_title: &'static str,
    /// The time at which the initial state was created.
    pub started_at_unix_seconds: u64,
}

/// Request metadata shared by every IPC command.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestContext {
    /// The contract version expected by the caller.
    pub api_version: u16,
    /// A caller-generated identifier used to correlate the response.
    pub request_id: String,
}

impl RequestContext {
    /// Validates the contract version and request identifier.
    pub fn validate(&self) -> Result<(), Box<ErrorDto>> {
        if self.api_version != API_VERSION {
            return Err(Box::new(ErrorDto::new(
                "UNSUPPORTED_API_VERSION",
                "apiVersion",
                "このAPIバージョンはサポートされていません。",
                false,
            )));
        }
        if self.request_id.is_empty() || self.request_id.len() > 128 {
            return Err(Box::new(ErrorDto::new(
                "INVALID_REQUEST_ID",
                "requestId",
                "requestIdは1〜128文字で指定してください。",
                false,
            )));
        }
        Ok(())
    }
}

/// Request for the initial application state.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootstrapRequest {
    /// The contract version expected by the caller.
    pub api_version: u16,
    /// A caller-generated identifier used to correlate the response.
    pub request_id: String,
}

impl BootstrapRequest {
    /// Returns the request metadata for validation and response correlation.
    #[must_use]
    pub fn context(&self) -> RequestContext {
        RequestContext {
            api_version: self.api_version,
            request_id: self.request_id.clone(),
        }
    }
}

/// Public, non-sensitive initial application state.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootstrapResponse {
    /// The product name to display.
    pub application_title: &'static str,
    /// The time at which the initial state was created.
    pub started_at_unix_seconds: u64,
}

impl From<Bootstrap> for BootstrapResponse {
    fn from(bootstrap: Bootstrap) -> Self {
        Self {
            application_title: bootstrap.application_title,
            started_at_unix_seconds: bootstrap.started_at_unix_seconds,
        }
    }
}

/// A structured error safe for presentation in the UI.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorDto {
    /// Stable machine-readable error code.
    pub code: String,
    /// Optional form field associated with the error.
    pub field_path: Option<String>,
    /// Stable localization key or short reason.
    pub reason: String,
    /// Whether retrying the same request may succeed.
    pub retryability: Retryability,
    /// Operation associated with the error, when one exists.
    pub operation_id: Option<String>,
    /// Non-sensitive details safe to log or display.
    pub safe_details: Option<String>,
}

impl ErrorDto {
    fn new(code: &str, field_path: &str, reason: &str, retryable: bool) -> Self {
        Self {
            code: code.to_owned(),
            field_path: Some(field_path.to_owned()),
            reason: reason.to_owned(),
            retryability: if retryable {
                Retryability::Retryable
            } else {
                Retryability::NotRetryable
            },
            operation_id: None,
            safe_details: None,
        }
    }
}

/// Indicates whether the caller may retry an operation.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Retryability {
    /// The same request may be attempted again.
    Retryable,
    /// The request must be corrected before retrying.
    NotRetryable,
    /// The result is unknown and must be reconciled first.
    Unknown,
}

/// Envelope returned by every IPC command.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseEnvelope<T> {
    /// Contract version used by the response.
    pub api_version: u16,
    /// Echoed request identifier.
    pub request_id: String,
    /// Successful result, mutually exclusive with `error`.
    pub result: Option<T>,
    /// Structured error, mutually exclusive with `result`.
    pub error: Option<ErrorDto>,
}

impl<T> ResponseEnvelope<T> {
    /// Creates a successful response envelope.
    #[must_use]
    pub fn success(request_id: String, result: T) -> Self {
        Self {
            api_version: API_VERSION,
            request_id,
            result: Some(result),
            error: None,
        }
    }

    /// Creates a failed response envelope without exposing transport details.
    #[must_use]
    pub fn failure(request_id: String, error: ErrorDto) -> Self {
        Self {
            api_version: API_VERSION,
            request_id,
            result: None,
            error: Some(error),
        }
    }
}

/// Event payload used for operation progress notifications.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationEvent {
    /// Operation identifier.
    pub operation_id: String,
    /// Instance identifier, when the operation targets an instance.
    pub instance_id: Option<String>,
    /// Instance revision observed by the operation.
    pub revision: u64,
    /// Monotonically increasing sequence number per operation.
    pub sequence: u64,
    /// Stable event kind.
    pub kind: OperationEventKind,
}

/// Stable kinds of non-sensitive operation progress.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OperationEventKind {
    /// An operation was accepted.
    Accepted,
    /// An operation is currently being processed.
    Progress,
    /// An operation completed successfully.
    Completed,
    /// An operation failed.
    Failed,
}

/// Versioned event envelope sent to the frontend.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventEnvelope<T> {
    /// Contract version used by the event.
    pub api_version: u16,
    /// Event payload.
    pub event: T,
}

/// Loads the initial application state without depending on a transport.
pub struct BootstrapService<C> {
    clock: C,
}

impl<C> BootstrapService<C>
where
    C: Clock,
{
    /// Creates the service with the supplied system clock port.
    #[must_use]
    pub const fn new(clock: C) -> Self {
        Self { clock }
    }

    /// Returns the initial state for a supported client.
    #[must_use]
    pub fn bootstrap(&self) -> Bootstrap {
        Bootstrap {
            application_title: application_title(),
            started_at_unix_seconds: self.clock.unix_seconds(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        API_VERSION, Bootstrap, BootstrapRequest, BootstrapService, Clock, RequestContext,
    };

    struct FixedClock;

    impl Clock for FixedClock {
        fn unix_seconds(&self) -> u64 {
            0
        }
    }

    #[test]
    fn returns_the_initial_application_state() {
        let service = BootstrapService::new(FixedClock);

        assert_eq!(
            service.bootstrap(),
            Bootstrap {
                application_title: "ComposeNest",
                started_at_unix_seconds: 0,
            }
        );
    }

    #[test]
    fn rejects_unknown_api_versions() {
        let request = RequestContext {
            api_version: API_VERSION + 1,
            request_id: "request-1".to_owned(),
        };

        assert_eq!(
            request.validate().unwrap_err().code,
            "UNSUPPORTED_API_VERSION"
        );
    }

    #[test]
    fn rejects_unknown_request_fields() {
        let request = serde_json::from_str::<BootstrapRequest>(
            r#"{"apiVersion":1,"requestId":"request-1","extra":"reject"}"#,
        );

        assert!(request.is_err());
    }

    #[test]
    fn serializes_the_contract_with_camel_case_fields() {
        let request = BootstrapRequest {
            api_version: API_VERSION,
            request_id: "request-1".to_owned(),
        };

        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"apiVersion":1,"requestId":"request-1"}"#
        );
    }
}
