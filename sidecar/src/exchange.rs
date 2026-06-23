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

/// Evaluate an accessor against one message of an exchange.
///
/// - `headers` → case-insensitive header lookup.
/// - `body` + `*` → the full body verbatim.
/// - `body` + `$...` → JSONPath; first match. Scalars render without quotes,
///   objects/arrays as compact JSON.
///
/// Returns `None` when the lookup fails (caller falls back to literal text).
pub fn eval_accessor(part: Part, accessor: &str, msg: &ExchangeMessage) -> Option<String> {
    match part {
        Part::Headers => msg
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(accessor))
            .map(|(_, v)| v.clone()),
        Part::Body => {
            if accessor == "*" {
                return Some(msg.body.clone());
            }
            if accessor.starts_with('$') {
                return eval_jsonpath(accessor, &msg.body);
            }
            // XPath and other accessors land in phase 2.
            None
        }
    }
}

fn eval_jsonpath(path: &str, body: &str) -> Option<String> {
    use serde_json_path::JsonPath;

    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let json_path = JsonPath::parse(path).ok()?;
    let matched = json_path.query(&value).first()?;
    Some(render_json_value(matched))
}

/// Render a matched JSON value: scalars without quotes, structures as compact JSON.
fn render_json_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Resolve every `{{ name.message.part.accessor }}` request-variable reference
/// in `text` against the per-document `cache`. References that are not
/// request-variable shaped, that point at an un-run request, or whose accessor
/// fails to resolve are left as their literal `{{...}}` text.
pub fn resolve_request_variables(text: &str, cache: &ExchangeCache) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("}}") else {
            // No closing braces — emit the remainder verbatim.
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = &after_open[..close];
        let literal = &rest[open..open + 2 + close + 2];

        match resolve_one(inner, cache) {
            Some(value) => out.push_str(&value),
            None => out.push_str(literal),
        }

        rest = &after_open[close + 2..];
    }

    out.push_str(rest);
    out
}

fn resolve_one(inner: &str, cache: &ExchangeCache) -> Option<String> {
    let reference = parse_request_var_ref(inner)?;
    let exchange = cache.get(reference.name)?;
    let msg = match reference.message {
        Message::Request => &exchange.request,
        Message::Response => &exchange.response,
    };
    eval_accessor(reference.part, reference.accessor, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(headers: &[(&str, &str)], body: &str) -> ExchangeMessage {
        ExchangeMessage {
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.to_string(),
        }
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let m = msg(&[("X-AuthToken", "abc123")], "");
        assert_eq!(
            eval_accessor(Part::Headers, "x-authtoken", &m),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn header_miss_returns_none() {
        let m = msg(&[("Content-Type", "application/json")], "");
        assert_eq!(eval_accessor(Part::Headers, "Missing", &m), None);
    }

    #[test]
    fn body_star_returns_full_body() {
        let m = msg(&[], "{\"a\":1}");
        assert_eq!(
            eval_accessor(Part::Body, "*", &m),
            Some("{\"a\":1}".to_string())
        );
    }

    #[test]
    fn jsonpath_scalar_renders_without_quotes() {
        let m = msg(&[], r#"{"token":"abc123"}"#);
        assert_eq!(
            eval_accessor(Part::Body, "$.token", &m),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn jsonpath_nested_and_array() {
        let m = msg(&[], r#"{"data":{"id":42}}"#);
        assert_eq!(
            eval_accessor(Part::Body, "$.data.id", &m),
            Some("42".to_string())
        );
        let arr = msg(&[], r#"[{"id":"first"},{"id":"second"}]"#);
        assert_eq!(
            eval_accessor(Part::Body, "$[0].id", &arr),
            Some("first".to_string())
        );
    }

    #[test]
    fn jsonpath_object_renders_compact_json() {
        let m = msg(&[], r#"{"data":{"id":42,"name":"x"}}"#);
        assert_eq!(
            eval_accessor(Part::Body, "$.data", &m),
            Some(r#"{"id":42,"name":"x"}"#.to_string())
        );
    }

    #[test]
    fn jsonpath_missing_returns_none() {
        let m = msg(&[], r#"{"token":"abc"}"#);
        assert_eq!(eval_accessor(Part::Body, "$.nope", &m), None);
    }

    #[test]
    fn jsonpath_on_invalid_json_returns_none() {
        let m = msg(&[], "not json");
        assert_eq!(eval_accessor(Part::Body, "$.token", &m), None);
    }

    fn sample_cache() -> ExchangeCache {
        let mut cache = ExchangeCache::new();
        cache.insert(
            "login".to_string(),
            Exchange {
                request: msg(&[("Content-Type", "application/json")], r#"{"user":"foo"}"#),
                response: msg(
                    &[("X-AuthToken", "tok-42")],
                    r#"{"data":{"token":"abc123"}}"#,
                ),
            },
        );
        cache
    }

    #[test]
    fn resolves_response_header() {
        let cache = sample_cache();
        assert_eq!(
            resolve_request_variables("{{login.response.headers.X-AuthToken}}", &cache),
            "tok-42"
        );
    }

    #[test]
    fn resolves_response_body_jsonpath() {
        let cache = sample_cache();
        assert_eq!(
            resolve_request_variables(
                "Authorization: Bearer {{login.response.body.$.data.token}}",
                &cache
            ),
            "Authorization: Bearer abc123"
        );
    }

    #[test]
    fn resolves_request_body_star() {
        let cache = sample_cache();
        assert_eq!(
            resolve_request_variables("{{login.request.body.*}}", &cache),
            r#"{"user":"foo"}"#
        );
    }

    #[test]
    fn unknown_request_left_literal() {
        let cache = sample_cache();
        let input = "{{missing.response.headers.X}}";
        assert_eq!(resolve_request_variables(input, &cache), input);
    }

    #[test]
    fn unresolved_accessor_left_literal() {
        let cache = sample_cache();
        let input = "{{login.response.body.$.nope}}";
        assert_eq!(resolve_request_variables(input, &cache), input);
    }

    #[test]
    fn non_reference_left_literal() {
        let cache = sample_cache();
        let input = "{{baseUrl}}/x {{$guid}}";
        assert_eq!(resolve_request_variables(input, &cache), input);
    }

    #[test]
    fn multiple_refs_in_one_string() {
        let cache = sample_cache();
        let input = "{{login.response.headers.X-AuthToken}}-{{login.response.body.$.data.token}}";
        assert_eq!(resolve_request_variables(input, &cache), "tok-42-abc123");
    }

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
