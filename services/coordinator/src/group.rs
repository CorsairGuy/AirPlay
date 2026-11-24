//! Group coordination and independent receiver adoption state.
use anyhow::{Result, bail};
use std::collections::BTreeMap;

const MAX_MEMBERS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockDomain {
    Ntp,
    Ptp(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupContext {
    uuid: u128,
    clock: ClockDomain,
    contains_discoverable_leader: bool,
}

impl GroupContext {
    pub fn new(uuid: u128, clock: ClockDomain, contains_discoverable_leader: bool) -> Result<Self> {
        if uuid == 0 || clock == ClockDomain::Ptp(0) {
            bail!("group and PTP clock IDs must be nonzero");
        }
        Ok(Self {
            uuid,
            clock,
            contains_discoverable_leader,
        })
    }
    pub fn contains_discoverable_leader(&self) -> bool {
        self.contains_discoverable_leader
    }
    pub fn uuid(&self) -> u128 {
        self.uuid
    }
    pub fn clock(&self) -> ClockDomain {
        self.clock
    }
}

/// A connection-generation capability. Tokens from another coordinator or an
/// earlier attempt cannot mutate a current member, even after remove/re-add.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    owner: u128,
    generation: u64,
    member: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    pub group: Option<u128>,
    pub is_group_leader: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Membership {
    Selected,
    Ready,
    Confirmed,
    Diverged,
}

#[derive(Clone, Debug)]
struct Member {
    attempt: Attempt,
    session: Option<u128>,
    observed: Option<Observation>,
}

/// Desired members are stored separately from session and observed readiness.
/// Membership changes to one target preserve all other attempts and sessions.
pub struct Coordinator {
    context: GroupContext,
    owner: u128,
    generation: u64,
    members: BTreeMap<String, Member>,
}

impl Coordinator {
    pub fn new(context: GroupContext) -> Self {
        Self {
            context,
            owner: rand::random(),
            generation: 0,
            members: BTreeMap::new(),
        }
    }
    pub fn context(&self) -> &GroupContext {
        &self.context
    }
    pub fn select(&mut self, uid: &str) -> Result<Attempt> {
        if let Some(member) = self.members.get(uid) {
            if member.attempt.generation != 0 {
                return Ok(member.attempt.clone());
            }
            return self.begin(uid);
        }
        if uid.is_empty() || uid.len() > 256 || self.members.len() >= MAX_MEMBERS {
            bail!("invalid member identity or group capacity exceeded");
        }
        self.begin(uid)
    }
    /// Explicit retry invalidates only this member's previous attempt.
    pub fn retry(&mut self, uid: &str) -> Result<Attempt> {
        if !self.members.contains_key(uid) {
            bail!("member is not selected");
        }
        self.begin(uid)
    }
    fn begin(&mut self, uid: &str) -> Result<Attempt> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("generation exhausted"))?;
        let attempt = Attempt {
            owner: self.owner,
            generation,
            member: uid.into(),
        };
        self.generation = generation;
        self.members.insert(
            uid.into(),
            Member {
                attempt: attempt.clone(),
                session: None,
                observed: None,
            },
        );
        Ok(attempt)
    }
    pub fn remove(&mut self, uid: &str) -> bool {
        self.members.remove(uid).is_some()
    }
    fn current(&mut self, attempt: &Attempt) -> Option<&mut Member> {
        self.members
            .get_mut(&attempt.member)
            .filter(|member| member.attempt == *attempt)
    }
    /// The clock is the domain actually negotiated by this connection.
    pub fn ready(&mut self, attempt: &Attempt, session: u128, clock: ClockDomain) -> Result<bool> {
        if session == 0 || clock != self.context.clock {
            bail!("invalid session or mismatched clock domain");
        }
        if self
            .members
            .values()
            .any(|m| m.attempt != *attempt && m.session == Some(session))
        {
            bail!("session identity already belongs to another member");
        }
        let Some(member) = self.current(attempt) else {
            return Ok(false);
        };
        if member.session.is_some_and(|existing| existing != session) {
            bail!("replacement session requires a new attempt");
        }
        member.session = Some(session);
        Ok(true)
    }
    pub fn observe(&mut self, attempt: &Attempt, observation: Observation) -> bool {
        let Some(member) = self.current(attempt) else {
            return false;
        };
        member.observed = Some(observation);
        true
    }
    /// Transport failure does not erase the user's desired selection. Retrying
    /// is explicit and gives the replacement connection a fresh Attempt.
    pub fn failed(&mut self, attempt: &Attempt) -> bool {
        let Some(member) = self.current(attempt) else {
            return false;
        };
        member.session = None;
        member.observed = None;
        // Retire the capability immediately: late ready/observe callbacks from
        // this connection must not resurrect it before retry().
        member.attempt.generation = 0;
        true
    }
    pub fn membership(&self, uid: &str) -> Option<Membership> {
        let member = self.members.get(uid)?;
        Some(if member.session.is_none() {
            Membership::Selected
        } else {
            match member.observed {
                Some(Observation {
                    group: Some(group),
                    is_group_leader: Some(false),
                }) if group == self.context.uuid => Membership::Confirmed,
                Some(Observation {
                    group: Some(group), ..
                }) if group != self.context.uuid => Membership::Diverged,
                Some(Observation {
                    is_group_leader: Some(true),
                    ..
                }) => Membership::Diverged,
                _ => Membership::Ready,
            }
        })
    }
    /// Only these members may be advertised as confirmed followers by a future
    /// adapter. This API is not connected to today's MRP or sender runtime.
    pub fn confirmed(&self) -> impl Iterator<Item = &str> {
        self.members
            .keys()
            .filter(|uid| self.membership(uid) == Some(Membership::Confirmed))
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> GroupContext {
        GroupContext::new(123, ClockDomain::Ptp(0xf842885f71750008), true).unwrap()
    }
    fn observation() -> Observation {
        Observation {
            group: Some(123),
            is_group_leader: Some(false),
        }
    }
    #[test]
    fn setup_success_does_not_claim_group_adoption() {
        let mut c = Coordinator::new(context());
        let a = c.select("pod1").unwrap();
        assert_eq!(c.membership("pod1"), Some(Membership::Selected));
        c.ready(&a, 9, context().clock()).unwrap();
        assert_eq!(c.confirmed().count(), 0);
        c.observe(
            &a,
            Observation {
                group: Some(456),
                is_group_leader: Some(true),
            },
        );
        assert_eq!(c.membership("pod1"), Some(Membership::Diverged));
        c.observe(&a, observation());
        assert_eq!(c.confirmed().collect::<Vec<_>>(), ["pod1"]);
    }
    #[test]
    fn remove_readd_and_retry_reject_late_callbacks() {
        let mut c = Coordinator::new(context());
        let old = c.select("pod").unwrap();
        c.remove("pod");
        let new = c.select("pod").unwrap();
        assert!(!c.ready(&old, 1, context().clock()).unwrap());
        assert!(!c.observe(&old, observation()));
        c.ready(&new, 2, context().clock()).unwrap();
        c.failed(&new);
        assert!(!c.ready(&new, 2, context().clock()).unwrap());
        let retry = c.retry("pod").unwrap();
        assert_ne!(retry, new);
        assert!(!c.failed(&new));
        assert!(c.ready(&retry, 3, context().clock()).unwrap());
    }
    #[test]
    fn another_member_does_not_restart_confirmed_session() {
        let mut c = Coordinator::new(context());
        let a = c.select("a").unwrap();
        c.ready(&a, 1, context().clock()).unwrap();
        c.observe(&a, observation());
        c.select("b").unwrap();
        c.remove("b");
        assert_eq!(c.select("a").unwrap(), a);
        assert_eq!(c.membership("a"), Some(Membership::Confirmed));
    }
    #[test]
    fn wrong_clock_duplicate_session_and_foreign_tokens_are_rejected() {
        let mut c = Coordinator::new(context());
        let a = c.select("a").unwrap();
        let b = c.select("b").unwrap();
        assert!(c.ready(&a, 1, ClockDomain::Ntp).is_err());
        c.ready(&a, 1, context().clock()).unwrap();
        assert!(c.ready(&b, 1, context().clock()).is_err());
        let mut other = Coordinator::new(context());
        other.select("a").unwrap();
        assert!(!other.observe(&a, observation()));
    }
    #[test]
    fn missing_role_never_confirms_and_observation_can_arrive_before_ready() {
        let mut c = Coordinator::new(context());
        let a = c.select("a").unwrap();
        c.observe(&a, observation());
        assert_eq!(c.membership("a"), Some(Membership::Selected));
        c.ready(&a, 1, context().clock()).unwrap();
        assert_eq!(c.membership("a"), Some(Membership::Confirmed));
        c.observe(
            &a,
            Observation {
                group: Some(123),
                is_group_leader: None,
            },
        );
        assert_eq!(c.membership("a"), Some(Membership::Ready));
    }
    #[test]
    fn selecting_failed_member_issues_new_capability_and_replacement_needs_retry() {
        let mut c = Coordinator::new(context());
        let old = c.select("a").unwrap();
        c.ready(&old, 1, context().clock()).unwrap();
        c.observe(&old, observation());
        assert!(c.ready(&old, 2, context().clock()).is_err());
        assert!(c.failed(&old));
        let new = c.select("a").unwrap();
        assert_ne!(old, new);
        assert!(!c.observe(&old, observation()));
        assert!(!c.ready(&old, 1, context().clock()).unwrap());
        c.ready(&new, 2, context().clock()).unwrap();
        assert_eq!(c.membership("a"), Some(Membership::Ready));
    }

    #[test]
    fn capacity_and_generation_exhaustion_do_not_evict_existing_members() {
        let mut c = Coordinator::new(context());
        for i in 0..MAX_MEMBERS {
            c.select(&format!("member{i}")).unwrap();
        }
        let first = c.select("member0").unwrap();
        assert!(c.select("overflow").is_err());
        assert_eq!(c.select("member0").unwrap(), first);
        c.generation = u64::MAX;
        assert!(c.retry("member0").is_err());
        assert_eq!(c.select("member0").unwrap(), first);
    }
}
