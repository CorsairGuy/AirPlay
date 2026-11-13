//! Typed output selection requests. Keep device and cluster-aware namespaces
//! separate: only discovery can map these identifiers to physical receivers.
use crate::{
    Error,
    schema::field as f,
    wire::{Node, Value},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub adding: Vec<String>,
    pub removing: Vec<String>,
    pub setting: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub devices: Changes,
    pub clusters: Changes,
}

fn identifiers(body: &Node, tag: u32) -> Result<Vec<String>, Error> {
    let mut ids = Vec::new();
    for (_, value) in body.0.iter().filter(|(field, _)| *field == tag) {
        let Value::Bytes(bytes) = value else {
            return Err(Error::Protobuf("invalid output identifier wire type"));
        };
        let id = std::str::from_utf8(bytes)
            .map_err(|_| Error::Protobuf("invalid output identifier UTF8"))?;
        if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) || ids.len() >= 64 {
            return Err(Error::Protobuf(
                "invalid output identifier or too many outputs",
            ));
        }
        ids.push(id.to_owned());
    }
    Ok(ids)
}

impl Request {
    /// Parse the message body (extension52 of protocol message48).
    /// An absent repeated setting field cannot encode an explicit empty reset.
    /// Do not infer reset, operation precedence or cluster membership here.
    pub fn parse(body: &Node) -> Result<Self, Error> {
        // Protobuf readers ignore unknown fields. Validate known fields only,
        // without accidentally treating future byte fields as device UIDs.
        Node(
            body.0
                .iter()
                .filter(|(tag, _)| (1..=7).contains(tag))
                .cloned()
                .collect(),
        )
        .validate("ModifyOutputContextRequestMessage")?;
        if body
            .number(f::ModifyOutputContextRequestMessage::r#type)
            .is_some_and(|kind| kind != 1)
        {
            return Err(Error::Protobuf("unsupported output context request type"));
        }
        Ok(Self {
            devices: Changes {
                adding: identifiers(body, f::ModifyOutputContextRequestMessage::addingDevices)?,
                removing: identifiers(body, f::ModifyOutputContextRequestMessage::removingDevices)?,
                setting: identifiers(body, f::ModifyOutputContextRequestMessage::settingDevices)?,
            },
            clusters: Changes {
                adding: identifiers(
                    body,
                    f::ModifyOutputContextRequestMessage::clusterAwareAddingDevices,
                )?,
                removing: identifiers(
                    body,
                    f::ModifyOutputContextRequestMessage::clusterAwareRemovingDevices,
                )?,
                setting: identifiers(
                    body,
                    f::ModifyOutputContextRequestMessage::clusterAwareSettingDevices,
                )?,
            },
        })
    }

    pub fn identifiers(&self) -> impl Iterator<Item = &str> {
        [
            &self.devices.adding,
            &self.devices.removing,
            &self.devices.setting,
            &self.clusters.adding,
            &self.clusters.removing,
            &self.clusters.setting,
        ]
        .into_iter()
        .flatten()
        .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_all_operations_and_namespaces() {
        let mut body = Node::default();
        body.int(1, 1);
        for tag in 2..=7 {
            body.text(tag, format!("uid{tag}"));
        }
        let request = Request::parse(&Node::parse(&body.encode()).unwrap()).unwrap();
        assert_eq!(
            request.devices,
            Changes {
                adding: vec!["uid2".into()],
                removing: vec!["uid3".into()],
                setting: vec!["uid4".into()]
            }
        );
        assert_eq!(
            request.clusters,
            Changes {
                adding: vec!["uid5".into()],
                removing: vec!["uid6".into()],
                setting: vec!["uid7".into()]
            }
        );
        assert_eq!(request.identifiers().count(), 6);
    }

    #[test]
    fn malformed_ids_cannot_disappear_into_an_empty_success() {
        for tag in 2..=7 {
            for value in [
                Value::Bytes(vec![255]),
                Value::Bytes(vec![]),
                Value::Bytes(b"x\ny".to_vec()),
                Value::Bytes(vec![b'x'; 257]),
                Value::Int(7),
            ] {
                assert!(Request::parse(&Node(vec![(tag, value)])).is_err());
            }
        }
    }

    #[test]
    fn bounds_repeated_ids_and_rejects_unsupported_context() {
        let mut body = Node::default();
        for _ in 0..64 {
            body.text(2, "receiver");
        }
        assert_eq!(Request::parse(&body).unwrap().devices.adding.len(), 64);
        body.text(2, "receiver");
        assert!(Request::parse(&body).is_err());
        assert!(Request::parse(Node::default().int(1, 2)).is_err());
        assert_eq!(
            Request::parse(&Node::default()).unwrap(),
            Request::default()
        );
    }
}
