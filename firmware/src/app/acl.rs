use alloc::{string::String, vec::Vec};
use core::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Admin,
    Deny,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Acl(Vec<([u8; 32], Role)>);

impl Acl {
    pub fn role(&self, key: &[u8; 32]) -> Option<Role> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, role)| *role)
    }

    pub fn set(&mut self, key: &str, role: &str) -> Result<(), ()> {
        let key = parse_key(key).ok_or(())?;
        let role = match role {
            "admin" => Some(Role::Admin),
            "deny" => Some(Role::Deny),
            "unset" => None,
            _ => return Err(()),
        };
        self.0.retain(|(k, _)| *k != key);
        if let Some(role) = role {
            self.0.push((key, role));
            self.0.sort_unstable_by_key(|(key, _)| *key);
        }
        Ok(())
    }

    pub fn write_config(&self, out: &mut String) {
        for (key, role) in &self.0 {
            out.push_str("acl.");
            for byte in key {
                let _ = write!(out, "{byte:02x}");
            }
            out.push_str(match role {
                Role::Admin => "=admin\n",
                Role::Deny => "=deny\n",
            });
        }
    }

    // Persist removals of profile-provided rules so they do not return at boot.
    pub fn write_overrides(&self, defaults: &Self, out: &mut String) {
        self.write_config(out);
        for (key, _) in &defaults.0 {
            if self.role(key).is_none() {
                out.push_str("acl.");
                for byte in key {
                    let _ = write!(out, "{byte:02x}");
                }
                out.push_str("=unset\n");
            }
        }
    }

    pub fn changed_keys<'a>(&'a self, other: &'a Self) -> impl Iterator<Item = &'a [u8; 32]> {
        self.0
            .iter()
            .chain(other.0.iter())
            .map(|(key, _)| key)
            .filter(|key| self.role(key) != other.role(key))
    }
}

fn parse_key(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.is_ascii() {
        return None;
    }
    let mut key = [0; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_replace_and_unset_without_duplicates() {
        let key = "AB".repeat(32);
        let mut acl = Acl::default();
        acl.set(&key, "admin").unwrap();
        acl.set(&key.to_ascii_lowercase(), "deny").unwrap();
        assert_eq!(acl.role(&[0xab; 32]), Some(Role::Deny));
        let mut text = String::new();
        acl.write_config(&mut text);
        assert_eq!(
            text,
            alloc::format!("acl.{}=deny\n", key.to_ascii_lowercase())
        );
        acl.set(&key, "unset").unwrap();
        assert_eq!(acl.role(&[0xab; 32]), None);
        acl.set(&key, "unset").unwrap();
    }

    #[test]
    fn malformed_rules_do_not_mutate_acl() {
        let key = "01".repeat(32);
        let mut acl = Acl::default();
        acl.set(&key, "deny").unwrap();
        for (key, role) in [
            ("01", "admin"),
            (&"zz".repeat(32), "admin"),
            (&key, "guest"),
            (&"é".repeat(32), "deny"),
        ] {
            assert!(acl.set(key, role).is_err());
        }
        assert_eq!(acl.role(&[1; 32]), Some(Role::Deny));
    }

    #[test]
    fn profile_removal_survives_serialization() {
        let key = "01".repeat(32);
        let mut defaults = Acl::default();
        defaults.set(&key, "admin").unwrap();
        let mut current = defaults.clone();
        current.set(&key, "unset").unwrap();
        let mut text = String::new();
        current.write_overrides(&defaults, &mut text);
        let mut restored = defaults;
        for line in text.lines() {
            let (key, role) = line.strip_prefix("acl.").unwrap().split_once('=').unwrap();
            restored.set(key, role).unwrap();
        }
        assert_eq!(restored.role(&[1; 32]), None);
    }

    #[test]
    fn removed_and_added_rules_are_reported_as_changes() {
        let mut before = Acl::default();
        before.set(&"01".repeat(32), "admin").unwrap();
        let mut after = Acl::default();
        after.set(&"02".repeat(32), "deny").unwrap();
        let keys: Vec<_> = before.changed_keys(&after).copied().collect();
        assert!(keys.contains(&[1; 32]));
        assert!(keys.contains(&[2; 32]));
    }
}
