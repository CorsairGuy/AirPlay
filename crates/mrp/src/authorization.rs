//! Route PIN sheet messages59/60/61, ported from Station Claude03d0277.
//! Pure wire handling: the connection owner handles prompt lifetime, correlation,
//! user cancellation and the sender's PinPairing exchange. No playback or I/O.
use crate::{
    Error,
    messages::envelope,
    schema::{field as f, kind},
    wire::Node,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Status {
    Ok = 0,
    AuthenticationFailed = 2,
    UnknownError = 5,
}

pub fn prompt(route: &Node, identifier: &str) -> Result<Node, Error> {
    if identifier.is_empty() || identifier.len() > 256 {
        return Err(Error::Protobuf("invalid route authorization identifier"));
    }
    route.validate("AVOutputDeviceDescriptor")?;
    let mut body = Node::default();
    body.node(f::PromptForRouteAuthorizationMessage::route, route)
        .int(f::PromptForRouteAuthorizationMessage::inputType, 2);
    Ok(envelope(
        kind::PROMPT_FOR_ROUTE_AUTHORIZATION_MESSAGE,
        &body,
        Some(identifier),
    ))
}

pub fn status(route: &Node, status: Status) -> Result<Node, Error> {
    route.validate("AVOutputDeviceDescriptor")?;
    let mut body = Node::default();
    body.node(f::PresentRouteAuthorizationStatusMessage::route, route)
        .int(
            f::PresentRouteAuthorizationStatusMessage::status,
            status as u64,
        );
    Ok(envelope(
        kind::PRESENT_ROUTE_AUTHORIZATION_STATUS_MESSAGE,
        &body,
        None,
    ))
}

pub struct Response {
    identifier: Option<String>,
    pin: Option<String>,
}
impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthorizationResponse { private }")
    }
}
impl Response {
    pub fn identifier(&self) -> Option<&str> {
        self.identifier.as_deref()
    }
    /// None is an empty/cancelled sheet, not a pairing failure or a default PIN.
    pub fn into_pin(self) -> Option<String> {
        self.pin
    }
}

pub fn response(message: &Node) -> Result<Response, Error> {
    message.validate("ProtocolMessage")?;
    if message.number(f::ProtocolMessage::r#type)
        != Some(kind::PROMPT_FOR_ROUTE_AUTHORIZATION_RESPONSE_MESSAGE as u64)
    {
        return Err(Error::Protobuf("not a route authorization response"));
    }
    let identifier = match message.get(f::ProtocolMessage::identifier) {
        None => None,
        Some(_) => Some(
            message
                .string(f::ProtocolMessage::identifier)
                .filter(|s| !s.is_empty() && s.len() <= 256)
                .ok_or(Error::Protobuf("invalid route authorization identifier"))?
                .to_owned(),
        ),
    };
    let tag =
        crate::schema::extension(kind::PROMPT_FOR_ROUTE_AUTHORIZATION_RESPONSE_MESSAGE).unwrap();
    let body = message.nested(tag)?;
    let field = f::PromptForRouteAuthorizationResponseMessage::response;
    let pin = match body.get(field) {
        None => None,
        Some(_) => {
            let value = body
                .string(field)
                .ok_or(Error::Protobuf("invalid PIN encoding"))?;
            if value.len() > 64 {
                return Err(Error::Protobuf("PIN response too long"));
            }
            let value = value.trim();
            if value.is_empty() {
                None
            } else if value.bytes().all(|b| b.is_ascii_digit()) {
                Some(value.to_owned())
            } else {
                return Err(Error::Protobuf("non-numeric route PIN"));
            }
        }
    };
    Ok(Response { identifier, pin })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn route() -> Node {
        let mut route = Node::default();
        route
            .text(f::AVOutputDeviceDescriptor::name, "Receiver")
            .text(f::AVOutputDeviceDescriptor::uniqueIdentifier, "receiver-id");
        route
    }
    fn reply(body: &Node) -> Node {
        envelope(
            kind::PROMPT_FOR_ROUTE_AUTHORIZATION_RESPONSE_MESSAGE,
            body,
            Some("request-id"),
        )
    }
    #[test]
    fn wire_tags_match_station_and_mediaremote_layout() {
        let prompt = prompt(&route(), "request-id").unwrap();
        prompt.validate("ProtocolMessage").unwrap();
        assert_eq!(prompt.number(1), Some(59));
        assert_eq!(prompt.nested(63).unwrap().number(2), Some(2));
        let status = status(&route(), Status::AuthenticationFailed).unwrap();
        status.validate("ProtocolMessage").unwrap();
        assert_eq!(status.number(1), Some(61));
        assert_eq!(status.nested(65).unwrap().number(2), Some(2));
        let mut body = Node::default();
        body.text(1, " 4821 ");
        let decoded = response(&Node::parse(&reply(&body).encode()).unwrap()).unwrap();
        assert_eq!(decoded.identifier(), Some("request-id"));
        assert_eq!(format!("{decoded:?}"), "AuthorizationResponse { private }");
        assert_eq!(decoded.into_pin().as_deref(), Some("4821"));
    }
    #[test]
    fn cancellation_is_distinct_from_malformed_or_wrong_messages() {
        assert!(
            response(&reply(&Node::default()))
                .unwrap()
                .into_pin()
                .is_none()
        );
        for value in ["", " "] {
            let mut body = Node::default();
            body.text(1, value);
            assert!(response(&reply(&body)).unwrap().into_pin().is_none());
        }
        for value in ["12x4".to_owned(), "1".repeat(65)] {
            let mut body = Node::default();
            body.text(1, value);
            assert!(response(&reply(&body)).is_err());
        }
        let mut body = Node::default();
        body.0.push((1, crate::wire::Value::Bytes(vec![255])));
        assert!(response(&reply(&body)).is_err());
        assert!(response(&prompt(&route(), "id").unwrap()).is_err());
        assert!(prompt(&route(), "").is_err());
    }
}
