//! The operations registry. `define_ops!` and `define_topics!` declare each
//! operation and topic once and generate the typed `Call` and `Topic` enums the
//! agent matches exhaustively, the `OPS` and `TOPICS` tables, and the schema
//! hooks `cargo xtask codegen` uses.

use std::fmt;

use serde_json::{Map, Value};

use crate::codes::ErrorCode;

/// A registry entry: the wire name, the policy capability that gates it, and the
/// protocol revision that introduced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpInfo {
    pub name: &'static str,
    pub capability: &'static str,
    pub since: u32,
}

/// Topics have the same registry fields as operations.
pub type TopicInfo = OpInfo;

/// Why a request's or subscription's payload couldn't be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// No operation or topic has this name.
    Unknown(String),
    /// The payload doesn't match the parameters type.
    BadParams { name: &'static str, reason: String },
}

impl DecodeError {
    /// The error code to answer with.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Unknown(_) => ErrorCode::UnknownOp,
            Self::BadParams { .. } => ErrorCode::BadRequest,
        }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(name) => write!(f, "unknown operation or topic `{name}`"),
            Self::BadParams { name, reason } => write!(f, "bad parameters for `{name}`: {reason}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// An absent payload decodes like `{}`, so operations without parameters accept
/// either.
pub(crate) fn normalize(data: Value) -> Value {
    if data.is_null() {
        Value::Object(Map::new())
    } else {
        data
    }
}

/// Schemas of one operation, for code generation.
#[cfg(feature = "schema")]
#[derive(Debug, Clone)]
pub struct OpSchema {
    pub info: OpInfo,
    pub params: schemars::Schema,
    pub result: schemars::Schema,
}

/// Schemas of one topic, for code generation.
#[cfg(feature = "schema")]
#[derive(Debug, Clone)]
pub struct TopicSchema {
    pub info: TopicInfo,
    pub params: schemars::Schema,
    pub event: schemars::Schema,
}

macro_rules! define_ops {
    ($(
        $(#[$attr:meta])*
        $variant:ident = $name:literal {
            params: $params:ty,
            result: $result:ty,
            capability: $capability:literal,
            since: $since:literal $(,)?
        }
    ),* $(,)?) => {
        /// A decoded request, one variant per operation, so a dispatcher that
        /// misses an operation doesn't compile.
        #[derive(Debug, Clone, PartialEq)]
        pub enum Call {
            $( $(#[$attr])* $variant($params), )*
        }

        /// Every operation, in declaration order.
        pub const OPS: &[$crate::OpInfo] = &[
            $( $crate::OpInfo { name: $name, capability: $capability, since: $since }, )*
        ];

        impl Call {
            /// Decodes a request's `op` and `data`.
            pub fn decode(op: &str, data: serde_json::Value) -> Result<Self, $crate::DecodeError> {
                let data = $crate::registry::normalize(data);
                match op {
                    $(
                        $name => serde_json::from_value(data).map(Self::$variant).map_err(|e| {
                            $crate::DecodeError::BadParams { name: $name, reason: e.to_string() }
                        }),
                    )*
                    other => Err($crate::DecodeError::Unknown(other.to_owned())),
                }
            }

            /// The operation's wire name.
            pub fn name(&self) -> &'static str {
                match self { $( Self::$variant(_) => $name, )* }
            }

            /// The policy capability that gates the operation.
            pub fn capability(&self) -> &'static str {
                match self { $( Self::$variant(_) => $capability, )* }
            }
        }

        /// Schemas of every operation; registers their types in `generator`.
        #[cfg(feature = "schema")]
        pub fn op_schemas(generator: &mut schemars::SchemaGenerator) -> Vec<$crate::OpSchema> {
            vec![$(
                $crate::OpSchema {
                    info: $crate::OpInfo { name: $name, capability: $capability, since: $since },
                    params: generator.subschema_for::<$params>(),
                    result: generator.subschema_for::<$result>(),
                },
            )*]
        }
    };
}

macro_rules! define_topics {
    ($(
        $(#[$attr:meta])*
        $variant:ident = $name:literal {
            params: $params:ty,
            event: $event:ty,
            capability: $capability:literal,
            since: $since:literal $(,)?
        }
    ),* $(,)?) => {
        /// A decoded subscription, one variant per topic.
        #[derive(Debug, Clone, PartialEq)]
        pub enum Topic {
            $( $(#[$attr])* $variant($params), )*
        }

        /// Every topic, in declaration order.
        pub const TOPICS: &[$crate::TopicInfo] = &[
            $( $crate::OpInfo { name: $name, capability: $capability, since: $since }, )*
        ];

        impl Topic {
            /// Decodes a subscription's `topic` and `data`.
            pub fn decode(topic: &str, data: serde_json::Value) -> Result<Self, $crate::DecodeError> {
                let data = $crate::registry::normalize(data);
                match topic {
                    $(
                        $name => serde_json::from_value(data).map(Self::$variant).map_err(|e| {
                            $crate::DecodeError::BadParams { name: $name, reason: e.to_string() }
                        }),
                    )*
                    other => Err($crate::DecodeError::Unknown(other.to_owned())),
                }
            }

            /// The topic's wire name.
            pub fn name(&self) -> &'static str {
                match self { $( Self::$variant(_) => $name, )* }
            }

            /// The policy capability that gates the topic.
            pub fn capability(&self) -> &'static str {
                match self { $( Self::$variant(_) => $capability, )* }
            }
        }

        /// Schemas of every topic; registers their types in `generator`.
        #[cfg(feature = "schema")]
        pub fn topic_schemas(generator: &mut schemars::SchemaGenerator) -> Vec<$crate::TopicSchema> {
            vec![$(
                $crate::TopicSchema {
                    info: $crate::OpInfo { name: $name, capability: $capability, since: $since },
                    params: generator.subschema_for::<$params>(),
                    event: generator.subschema_for::<$event>(),
                },
            )*]
        }
    };
}

pub(crate) use {define_ops, define_topics};
