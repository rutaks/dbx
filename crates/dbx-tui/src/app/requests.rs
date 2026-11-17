//! The request table: what each in-flight Core call is for, with a generation
//! per target so a superseded reply can be told from a current one
//! (design sections 3 and 11).

use std::collections::HashMap;

use crate::core::{ConnectionId, TabId};

/// Identifies one Core call from issue to reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

/// The thing a request loads or does. Each target has its own generation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    /// The connection list.
    Connections,
    /// A query tab's run.
    Run(TabId),
    /// The children of one sidebar node, addressed by path.
    Children(Vec<String>),
    /// A test of the connection form with this serial (one form at a time, but
    /// a closed form's reply must not reach the next one).
    TestConnection(u64),
    /// A save from the connection form with this serial.
    SaveConnection(u64),
    /// The removal of one saved connection.
    RemoveConnection(ConnectionId),
}

#[derive(Debug, Default)]
pub struct Requests {
    next_id: u64,
    generations: HashMap<Target, u64>,
    pending: HashMap<RequestId, (Target, u64)>,
}

impl Requests {
    /// Starts a request for `target`. Any earlier request for the same target
    /// becomes stale.
    pub fn begin(&mut self, target: Target) -> RequestId {
        let generation = self.bump(&target);
        self.next_id += 1;
        let id = RequestId(self.next_id);
        self.pending.insert(id, (target, generation));
        id
    }

    /// Makes whatever is in flight for `target` stale without starting
    /// anything new (cancel, closing a tab).
    pub fn supersede(&mut self, target: Target) {
        self.bump(&target);
    }

    /// Takes a reply's request off the table. Returns its target when the
    /// reply is current, and `None` when it is stale, already taken or
    /// unknown: the caller drops it without touching state.
    pub fn finish(&mut self, id: RequestId) -> Option<Target> {
        let (target, generation) = self.pending.remove(&id)?;
        (self.generations.get(&target) == Some(&generation)).then_some(target)
    }

    fn bump(&mut self, target: &Target) -> u64 {
        let g = self.generations.entry(target.clone()).or_insert(0);
        *g += 1;
        *g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_to_the_latest_request_for_a_target_is_current() {
        let mut t = Requests::default();
        let id = t.begin(Target::Connections);
        assert_eq!(t.finish(id), Some(Target::Connections));
    }

    #[test]
    fn beginning_a_second_request_for_a_target_makes_the_first_stale() {
        let mut t = Requests::default();
        let old = t.begin(Target::Connections);
        let new = t.begin(Target::Connections);
        assert_eq!(t.finish(old), None);
        assert_eq!(t.finish(new), Some(Target::Connections));
    }

    #[test]
    fn superseding_a_target_makes_its_in_flight_request_stale() {
        let mut t = Requests::default();
        let id = t.begin(Target::Run(TabId(1)));
        t.supersede(Target::Run(TabId(1)));
        assert_eq!(t.finish(id), None);
    }

    #[test]
    fn generations_are_per_target_not_global() {
        let mut t = Requests::default();
        let run1 = t.begin(Target::Run(TabId(1)));
        let _run2 = t.begin(Target::Run(TabId(2)));
        let children = t.begin(Target::Children(vec!["pg".into()]));
        t.supersede(Target::Run(TabId(2)));
        assert_eq!(t.finish(run1), Some(Target::Run(TabId(1))));
        assert_eq!(t.finish(children), Some(Target::Children(vec!["pg".into()])));
    }

    #[test]
    fn a_reply_is_accepted_once_and_unknown_ids_are_stale() {
        let mut t = Requests::default();
        let id = t.begin(Target::Connections);
        assert!(t.finish(id).is_some());
        assert_eq!(t.finish(id), None);
        assert_eq!(t.finish(RequestId(999)), None);
    }

    #[test]
    fn request_ids_are_unique() {
        let mut t = Requests::default();
        assert_ne!(t.begin(Target::Connections), t.begin(Target::Connections));
    }
}
