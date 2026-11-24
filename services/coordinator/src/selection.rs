//! Resolve a typed output-selection request against the discovered catalog.
//! Protocol decoding belongs to the caller; this module owns selection policy.
use anyhow::Result;
use std::collections::BTreeSet;

use crate::outputs::OutputSnapshot;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub adding: Vec<String>,
    pub removing: Vec<String>,
    pub setting: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub devices: Changes,
    pub groups: Changes,
}

/// Committed logical output selection. Proposals remain side-effect-free until
/// the runtime confirms that the corresponding session changes were applied.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    selected: Vec<String>,
}

impl State {
    pub fn selected(&self) -> &[String] {
        &self.selected
    }

    pub fn commit(&mut self, selected: Vec<String>) {
        self.selected = selected;
    }
}

/// Resolve replacement/add/remove intent into a complete desired selection.
/// Every requested identifier is validated before any precedence is applied.
pub fn resolve(
    request: &Request,
    catalog: &impl OutputSnapshot,
    local_ids: &[String],
    current: &[String],
) -> Result<Vec<String>> {
    let canonical = |ids: &[String]| -> Result<Vec<String>> {
        ids.iter()
            .filter(|id| !local_ids.iter().any(|local| same_uid(local, id)))
            .map(|id| catalog.resolve_uid(id))
            .collect()
    };

    let adding = canonical(&request.devices.adding)?;
    let group_adding = canonical(&request.groups.adding)?;
    let removing = canonical(&request.devices.removing)?;
    let group_removing = canonical(&request.groups.removing)?;
    let setting = canonical(&request.devices.setting)?;
    let group_setting = canonical(&request.groups.setting)?;

    let replacing = !request.devices.setting.is_empty() || !request.groups.setting.is_empty();
    let mut selected: BTreeSet<String> = if replacing {
        setting.into_iter().chain(group_setting).collect()
    } else {
        canonical(current)?.into_iter().collect()
    };
    selected.extend(adding.into_iter().chain(group_adding));
    for id in removing.into_iter().chain(group_removing) {
        selected.remove(&id);
    }

    let desired: Vec<_> = selected.into_iter().collect();
    catalog.receivers(&desired)?;
    Ok(desired)
}

fn same_uid(a: &str, b: &str) -> bool {
    let key = |uid: &str| {
        let bytes = uid.as_bytes();
        let uuid = bytes.len() == 36
            && bytes.iter().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    *b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            });
        if uuid {
            uid.to_ascii_lowercase()
        } else {
            uid.to_owned()
        }
    };
    key(a) == key(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Catalog;
    impl OutputSnapshot for Catalog {
        type Route = String;

        fn resolve_uid(&self, uid: &str) -> Result<String> {
            match uid {
                "row-a" | "group-a" => Ok("row-a".into()),
                "row-b" | "group-b" => Ok("row-b".into()),
                _ => anyhow::bail!("unknown output"),
            }
        }

        fn route(&self, uid: &str) -> Result<Self::Route> {
            self.resolve_uid(uid)
        }

        fn receivers(&self, selected: &[String]) -> Result<Vec<crate::outputs::ReceiverEndpoint>> {
            selected
                .iter()
                .try_for_each(|id| self.resolve_uid(id).map(|_| ()))?;
            Ok(Vec::new())
        }

        fn merge_pinned(&self, _: Vec<Self::Route>) -> Result<Self> {
            Ok(Catalog)
        }
    }

    #[test]
    fn setting_add_remove_and_alias_precedence_resolve_complete_selection() {
        let catalog = Catalog;
        let request = Request {
            devices: Changes {
                adding: vec!["row-b".into()],
                removing: vec!["row-a".into()],
                setting: vec![],
            },
            groups: Changes {
                adding: vec!["group-a".into()],
                removing: vec![],
                setting: vec![],
            },
        };
        assert_eq!(
            resolve(&request, &catalog, &[], &["row-a".into()]).unwrap(),
            vec!["row-b".to_owned()]
        );

        let replacement = Request {
            devices: Changes {
                setting: vec!["row-a".into()],
                ..Changes::default()
            },
            groups: Changes {
                adding: vec!["group-b".into()],
                ..Changes::default()
            },
        };
        assert_eq!(
            resolve(&replacement, &catalog, &[], &["row-b".into()]).unwrap(),
            vec!["row-a".to_owned(), "row-b".to_owned()]
        );
    }

    #[test]
    fn local_output_is_filtered_and_every_identifier_is_validated() {
        let catalog = Catalog;
        let local_ids = vec!["ABCDEF01-2345-6789-ABCD-EF0123456789".into()];
        let request = Request {
            devices: Changes {
                setting: vec!["abcdef01-2345-6789-abcd-ef0123456789".into()],
                ..Changes::default()
            },
            ..Request::default()
        };
        assert!(
            resolve(&request, &catalog, &local_ids, &["row-a".into()])
                .unwrap()
                .is_empty()
        );

        let invalid = Request {
            devices: Changes {
                adding: vec!["missing".into()],
                setting: vec!["row-a".into()],
                ..Changes::default()
            },
            ..Request::default()
        };
        assert!(resolve(&invalid, &catalog, &[], &[]).is_err());
    }

    #[test]
    fn state_commits_only_an_explicitly_accepted_proposal() {
        let catalog = Catalog;
        let mut state = State::default();
        let request = Request {
            devices: Changes {
                setting: vec!["row-a".into()],
                ..Changes::default()
            },
            ..Request::default()
        };
        let proposal = resolve(&request, &catalog, &[], state.selected()).unwrap();
        assert!(state.selected().is_empty());
        state.commit(proposal);
        assert_eq!(state.selected().len(), 1);
        assert_eq!(state.selected()[0], "row-a");
    }
}
