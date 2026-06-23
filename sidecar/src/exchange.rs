//! Request-variable support: cached request/response exchanges and the
//! `{{ name.(request|response).(body|headers).accessor }}` reference grammar.

use std::collections::HashMap;

/// One side (request or response) of an HTTP exchange, captured for later
/// reference by name. Headers preserve order; lookup is case-insensitive.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExchangeMessage {
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// A named request together with what was actually sent and received.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct Exchange {
    pub request: ExchangeMessage,
    pub response: ExchangeMessage,
}

/// Per-document cache of named exchanges.
#[allow(dead_code)]
pub type ExchangeCache = HashMap<String, Exchange>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    Request,
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Body,
    Headers,
}

/// A parsed `name.message.part.accessor` reference (the inside of `{{ }}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestVarRef<'a> {
    pub name: &'a str,
    pub message: Message,
    pub part: Part,
    /// `*`, a JSONPath/XPath (for body), or a header name. May contain dots.
    pub accessor: &'a str,
}

/// Parse the inner text of a `{{ ... }}` placeholder as a request-variable
/// reference. Returns `None` for anything that is not a well-formed reference
/// (so it can fall through to ordinary variable handling).
pub fn parse_request_var_ref(inner: &str) -> Option<RequestVarRef<'_>> {
    let inner = inner.trim();
    // accessor may contain dots, so only split the first three segments off.
    let mut parts = inner.splitn(4, '.');
    let name = parts.next()?.trim();
    let message = parts.next()?.trim();
    let part = parts.next()?.trim();
    let accessor = parts.next()?.trim();

    if name.is_empty() || accessor.is_empty() {
        return None;
    }

    let message = match message {
        "request" => Message::Request,
        "response" => Message::Response,
        _ => return None,
    };
    let part = match part {
        "body" => Part::Body,
        "headers" => Part::Headers,
        _ => return None,
    };

    Some(RequestVarRef {
        name,
        message,
        part,
        accessor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_reference() {
        let r = parse_request_var_ref("login.response.headers.X-AuthToken").unwrap();
        assert_eq!(r.name, "login");
        assert_eq!(r.message, Message::Response);
        assert_eq!(r.part, Part::Headers);
        assert_eq!(r.accessor, "X-AuthToken");
    }

    #[test]
    fn parses_request_message() {
        let r = parse_request_var_ref("login.request.body.*").unwrap();
        assert_eq!(r.message, Message::Request);
        assert_eq!(r.part, Part::Body);
        assert_eq!(r.accessor, "*");
    }

    #[test]
    fn accessor_keeps_dots() {
        let r = parse_request_var_ref("login.response.body.$.data.token").unwrap();
        assert_eq!(r.accessor, "$.data.token");
    }

    #[test]
    fn rejects_unknown_message() {
        assert!(parse_request_var_ref("login.foo.body.*").is_none());
    }

    #[test]
    fn rejects_unknown_part() {
        assert!(parse_request_var_ref("login.response.cookies.x").is_none());
    }

    #[test]
    fn rejects_missing_accessor() {
        assert!(parse_request_var_ref("login.response.body").is_none());
    }

    #[test]
    fn rejects_plain_variable() {
        assert!(parse_request_var_ref("baseUrl").is_none());
        assert!(parse_request_var_ref("$guid").is_none());
    }
}
