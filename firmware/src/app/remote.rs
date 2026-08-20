use mcrs_protocol::{PUB_KEY_SIZE, Path};

pub(crate) const MAX_REMOTE_LOGINS: usize = 4;
pub(crate) const MAX_REMOTE_SESSIONS: usize = MAX_REMOTE_LOGINS * 2;
const REMOTE_LOGIN_TTL_MS: u64 = 10 * 60 * 1000;

type RemoteLoginEntries = [Option<RemoteLogin>; MAX_REMOTE_LOGINS];

#[derive(Clone)]
struct RemoteLogin {
    public_key: [u8; PUB_KEY_SIZE],
    shared_secret: [u8; 32],
    last_seen_ms: u64,
    last_timestamp: u32,
    reply_path: Path,
}

#[derive(Clone)]
pub struct RemoteSession {
    pub public_key: [u8; PUB_KEY_SIZE],
    pub shared_secret: [u8; 32],
    pub privilege: RemotePrivilege,
    pub reply_path: Path,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemotePrivilege {
    Guest,
    Admin,
}

pub struct RemoteLoginTable {
    admin_entries: RemoteLoginEntries,
    guest_entries: RemoteLoginEntries,
}

impl RemoteLoginTable {
    pub const fn new() -> Self {
        Self {
            admin_entries: [const { None }; MAX_REMOTE_LOGINS],
            guest_entries: [const { None }; MAX_REMOTE_LOGINS],
        }
    }

    pub fn authenticate(
        &mut self,
        public_key: &[u8; PUB_KEY_SIZE],
        shared_secret: &[u8; 32],
        privilege: RemotePrivilege,
        last_timestamp: u32,
        now_ms: u64,
        reply_path: &Path,
    ) {
        self.prune(now_ms);

        // A public key represents one logical session. Remove any prior entry
        // from both privilege tables so reauthentication upgrades or
        // downgrades the session instead of leaving contradictory copies.
        remove_login(&mut self.admin_entries, public_key);
        remove_login(&mut self.guest_entries, public_key);

        let entries = self.entries_mut(privilege);
        let index = first_empty_index(entries).unwrap_or_else(|| oldest_index(entries));
        entries[index] = Some(RemoteLogin {
            public_key: *public_key,
            shared_secret: *shared_secret,
            last_seen_ms: now_ms,
            last_timestamp,
            reply_path: reply_path.clone(),
        });
    }

    pub fn privilege_for(
        &mut self,
        public_key: &[u8; PUB_KEY_SIZE],
        now_ms: u64,
    ) -> Option<RemotePrivilege> {
        self.prune(now_ms);

        if let Some(login) = find_login_mut(&mut self.admin_entries, public_key) {
            login.last_seen_ms = now_ms;
            return Some(RemotePrivilege::Admin);
        }

        if let Some(login) = find_login_mut(&mut self.guest_entries, public_key) {
            login.last_seen_ms = now_ms;
            return Some(RemotePrivilege::Guest);
        }

        None
    }

    pub fn sessions_matching_source_hash(
        &mut self,
        source_hash: u8,
        now_ms: u64,
    ) -> [Option<RemoteSession>; MAX_REMOTE_SESSIONS] {
        self.prune(now_ms);

        let mut out = core::array::from_fn(|_| None);
        let mut index = 0;
        append_matching_sessions(
            &mut out,
            &mut index,
            &self.admin_entries,
            source_hash,
            RemotePrivilege::Admin,
        );
        append_matching_sessions(
            &mut out,
            &mut index,
            &self.guest_entries,
            source_hash,
            RemotePrivilege::Guest,
        );

        out
    }

    pub fn accept_newer_timestamp(
        &mut self,
        public_key: &[u8; PUB_KEY_SIZE],
        privilege: RemotePrivilege,
        timestamp: u32,
        now_ms: u64,
    ) -> bool {
        self.prune(now_ms);

        if let Some(login) = find_login_mut(self.entries_mut(privilege), public_key) {
            if timestamp <= login.last_timestamp {
                return false;
            }

            login.last_timestamp = timestamp;
            login.last_seen_ms = now_ms;
            return true;
        }

        false
    }

    fn prune(&mut self, now_ms: u64) {
        prune_entries(&mut self.admin_entries, now_ms);
        prune_entries(&mut self.guest_entries, now_ms);
    }

    fn entries_mut(&mut self, privilege: RemotePrivilege) -> &mut RemoteLoginEntries {
        match privilege {
            RemotePrivilege::Admin => &mut self.admin_entries,
            RemotePrivilege::Guest => &mut self.guest_entries,
        }
    }
}

fn prune_entries(entries: &mut RemoteLoginEntries, now_ms: u64) {
    for entry in entries {
        if entry
            .as_ref()
            .is_some_and(|login| now_ms.saturating_sub(login.last_seen_ms) > REMOTE_LOGIN_TTL_MS)
        {
            *entry = None;
        }
    }
}

fn find_login_mut<'a>(
    entries: &'a mut RemoteLoginEntries,
    public_key: &[u8; PUB_KEY_SIZE],
) -> Option<&'a mut RemoteLogin> {
    entries
        .iter_mut()
        .flatten()
        .find(|login| &login.public_key == public_key)
}

fn remove_login(entries: &mut RemoteLoginEntries, public_key: &[u8; PUB_KEY_SIZE]) {
    if let Some(entry) = entries.iter_mut().find(|entry| {
        entry
            .as_ref()
            .is_some_and(|login| &login.public_key == public_key)
    }) {
        *entry = None;
    }
}

fn append_matching_sessions(
    out: &mut [Option<RemoteSession>; MAX_REMOTE_SESSIONS],
    out_index: &mut usize,
    entries: &RemoteLoginEntries,
    source_hash: u8,
    privilege: RemotePrivilege,
) {
    for login in entries
        .iter()
        .flatten()
        .filter(|login| login.public_key[0] == source_hash)
    {
        out[*out_index] = Some(RemoteSession {
            public_key: login.public_key,
            shared_secret: login.shared_secret,
            privilege,
            reply_path: login.reply_path.clone(),
        });
        *out_index += 1;
    }
}

fn first_empty_index(entries: &RemoteLoginEntries) -> Option<usize> {
    entries.iter().position(Option::is_none)
}

fn oldest_index(entries: &RemoteLoginEntries) -> usize {
    let mut oldest_index = 0;
    let mut oldest_seen = u64::MAX;

    for (index, entry) in entries.iter().enumerate() {
        let Some(login) = entry else {
            return index;
        };

        if login.last_seen_ms < oldest_seen {
            oldest_index = index;
            oldest_seen = login.last_seen_ms;
        }
    }

    oldest_index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reauthentication_moves_session_between_privilege_tables() {
        let mut table = RemoteLoginTable::new();
        let public_key = [7; PUB_KEY_SIZE];
        let shared_secret = [9; 32];

        table.authenticate(
            &public_key,
            &shared_secret,
            RemotePrivilege::Guest,
            10,
            100,
            &Path::empty(),
        );
        table.authenticate(
            &public_key,
            &shared_secret,
            RemotePrivilege::Admin,
            20,
            200,
            &Path::empty(),
        );

        assert_eq!(
            table.privilege_for(&public_key, 201),
            Some(RemotePrivilege::Admin)
        );
        assert_eq!(table.guest_entries.iter().flatten().count(), 0);
        assert_eq!(table.admin_entries.iter().flatten().count(), 1);
        assert!(!table.accept_newer_timestamp(&public_key, RemotePrivilege::Guest, 21, 202));
        assert!(!table.accept_newer_timestamp(&public_key, RemotePrivilege::Admin, 20, 202));
        assert!(table.accept_newer_timestamp(&public_key, RemotePrivilege::Admin, 21, 202));

        table.authenticate(
            &public_key,
            &shared_secret,
            RemotePrivilege::Guest,
            30,
            300,
            &Path::empty(),
        );
        assert_eq!(
            table.privilege_for(&public_key, 301),
            Some(RemotePrivilege::Guest)
        );
        assert_eq!(table.admin_entries.iter().flatten().count(), 0);
        assert_eq!(table.guest_entries.iter().flatten().count(), 1);
    }
}
