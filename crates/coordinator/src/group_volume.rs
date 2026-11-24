//! Proportional group-volume policy. This is not Apple's undocumented formula.
//! A group target scales the loudest member to the requested level, preserving
//! relative levels and remembering them across a complete mute.
//! Ported from Station airplay_mrp/group_volume.rs. Supply observed normalized
//! member levels, not preserve-volume placeholder values. To avoid committing
//! balance after rejected control, plan on a clone and retain it only after
//! the caller accepts the complete batch. This policy performs no transport I/O.
use anyhow::{Result, bail};
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub struct GroupVolume {
    nonzero: BTreeMap<String, f32>,
}

impl GroupVolume {
    pub fn plan(&mut self, current: &[(String, f32)], target: f32) -> Result<Vec<(String, f32)>> {
        if !target.is_finite()
            || !(0.0..=1.0).contains(&target)
            || current.is_empty()
            || current.len() > 64
        {
            bail!("invalid group volume")
        }
        let mut levels = BTreeMap::new();
        for (uid, value) in current {
            if uid.is_empty()
                || uid.len() > 256
                || uid.chars().any(char::is_control)
                || !value.is_finite()
                || !(0.0..=1.0).contains(value)
                || levels.insert(uid.clone(), *value).is_some()
            {
                bail!("invalid group member")
            }
        }
        let peak = levels.values().copied().fold(0.0_f32, f32::max);
        if peak > 0.0 {
            self.nonzero = levels
                .iter()
                .map(|(uid, value)| (uid.clone(), value / peak))
                .collect();
        } else if !levels.keys().eq(self.nonzero.keys()) {
            // A new entirely muted group has no observed balance to recover.
            self.nonzero = levels.keys().map(|uid| (uid.clone(), 1.0)).collect();
        }
        Ok(current
            .iter()
            .map(|(uid, _)| (uid.clone(), (self.nonzero[uid] * target).clamp(0.0, 1.0)))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair(a: f32, b: f32) -> Vec<(String, f32)> {
        vec![("local".into(), a), ("pod".into(), b)]
    }

    #[test]
    fn supports_generic_groups_beyond_station_limit_and_rejects_bad_ids() {
        let members: Vec<_> = (0..64).map(|i| (format!("member{i}"), 0.3)).collect();
        let mut group = GroupVolume::default();
        assert_eq!(group.plan(&members, 0.2).unwrap().len(), 64);
        assert!(group.plan(&[("bad\nuid".into(), 0.1)], 0.2).is_err());
        let mut too_many = members;
        too_many.push(("overflow".into(), 0.2));
        assert!(group.plan(&too_many, 0.2).is_err());
    }
    #[test]
    fn group_scales_unequal_members_and_recovers_balance_after_mute() {
        let mut group = GroupVolume::default();
        let scaled = group.plan(&pair(0.1, 0.3), 0.2).unwrap();
        assert!((scaled[0].1 - 0.2 / 3.0).abs() < 1e-6);
        assert_eq!(scaled[1].1, 0.2);
        let muted = group.plan(&scaled, 0.0).unwrap();
        assert_eq!(muted, pair(0.0, 0.0));
        let restored = group.plan(&muted, 0.2).unwrap();
        assert_eq!(restored, scaled);
    }

    #[test]
    fn individual_adjustment_and_new_membership_replace_previous_balance() {
        let mut group = GroupVolume::default();
        group.plan(&pair(0.1, 0.3), 0.0).unwrap();
        assert_eq!(group.plan(&pair(0.2, 0.1), 0.4).unwrap(), pair(0.4, 0.2));
        let mut changed = pair(0.0, 0.0);
        changed[1].0 = "new-pod".into();
        assert_eq!(group.plan(&changed, 0.2).unwrap()[0].1, 0.2);
        assert_eq!(group.plan(&changed, 0.2).unwrap()[1].1, 0.2);
    }

    #[test]
    fn invalid_requests_do_not_poison_saved_balance() {
        let mut group = GroupVolume::default();
        let initial = group.plan(&pair(0.1, 0.3), 0.0).unwrap();
        assert!(group.plan(&pair(0.0, 0.0), f32::NAN).is_err());
        assert!(group.plan(&pair(0.0, 2.0), 0.2).is_err());
        let restored = group.plan(&initial, 0.3).unwrap();
        assert!((restored[0].1 - 0.1).abs() < 1e-6);
        assert_eq!(restored[1].1, 0.3);
    }
}
