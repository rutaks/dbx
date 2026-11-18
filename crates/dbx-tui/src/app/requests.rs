//! The request table: what each in-flight Core call is for, with a generation
//! per target so a superseded reply can be told from a current one
//! (design sections 3 and 11).

use std::collections::HashMap;

use super::tree::NodeId;
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
    Children(NodeId),
    /// A test of the connection form with this serial (one form at a time, but
    /// a closed form's reply must not reach the next one).
    TestConnection(u64),
    /// A save from the connection form with this serial.
    SaveConnection(u64),
    /// The removal of one saved connection.
    RemoveConnection(ConnectionId),
}

/// What a reply is, by the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    /// The reply to the latest request for its target.
    Current(Target),
    /// The reply to a request that was superseded.
    Stale(Target),
    /// Already taken, forgotten, or never issued.
    Unknown,
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

    /// Stops tracking `target`: its generation is dropped and every request
    /// still in flight for it becomes unknown, so its reply is dropped as
    /// stale. Returns those requests, for the caller to abandon. Used when a
    /// node collapses or its connection goes away, so the table does not grow
    /// with every node ever opened. (A later `begin` starts from generation 1
    /// again, which is safe because no request of the old life is left.)
    pub fn forget(&mut self, target: &Target) -> Vec<RequestId> {
        self.forget_where(|t| t == target)
    }

    /// `forget` for every tracked target `gone` is true of.
    pub fn forget_where(&mut self, gone: impl Fn(&Target) -> bool) -> Vec<RequestId> {
        self.generations.retain(|t, _| !gone(t));
        let mut dropped: Vec<RequestId> = self.pending.iter().filter(|(_, (t, _))| gone(t)).map(|(id, _)| *id).collect();
        dropped.sort_by_key(|id| id.0);
        for id in &dropped {
            self.pending.remove(id);
        }
        dropped
    }

    /// How many targets have a generation: what the table is holding on to.
    pub fn tracked(&self) -> usize {
        self.generations.len()
    }

    /// Takes a reply's request off the table. Returns its target when the
    /// reply is current, and `None` when it is stale, already taken or
    /// unknown: the caller drops it without touching state.
    pub fn finish(&mut self, id: RequestId) -> Option<Target> {
        match self.settle(id) {
            Settled::Current(target) => Some(target),
            _ => None,
        }
    }

    /// `finish`, telling a stale reply (its target is known, but a newer
    /// request or a cancel superseded it) from an unknown one. A stale reply
    /// must not change what is shown; it can still say that the work it
    /// carried is over (a cancelled run has ended).
    pub fn settle(&mut self, id: RequestId) -> Settled {
        let Some((target, generation)) = self.pending.remove(&id) else { return Settled::Unknown };
        if self.generations.get(&target) == Some(&generation) {
            Settled::Current(target)
        } else {
            Settled::Stale(target)
        }
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
        let node = Target::Children(NodeId::Connection(ConnectionId("pg".into())));
        let children = t.begin(node.clone());
        t.supersede(Target::Run(TabId(2)));
        assert_eq!(t.finish(run1), Some(Target::Run(TabId(1))));
        assert_eq!(t.finish(children), Some(node));
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
    fn settling_tells_a_stale_reply_from_an_unknown_one() {
        let mut t = Requests::default();
        let old = t.begin(Target::Run(TabId(1)));
        let new = t.begin(Target::Run(TabId(1)));
        assert_eq!(t.settle(old), Settled::Stale(Target::Run(TabId(1))));
        assert_eq!(t.settle(old), Settled::Unknown, "taken once");
        assert_eq!(t.settle(new), Settled::Current(Target::Run(TabId(1))));
        let forgotten = t.begin(node("a"));
        t.forget(&node("a"));
        assert_eq!(t.settle(forgotten), Settled::Unknown, "a forgotten target's reply is not even stale");
    }

    #[test]
    fn request_ids_are_unique() {
        let mut t = Requests::default();
        assert_ne!(t.begin(Target::Connections), t.begin(Target::Connections));
    }

    fn node(name: &str) -> Target {
        Target::Children(NodeId::Connection(ConnectionId(name.into())))
    }

    #[test]
    fn forgetting_a_target_drops_its_generation_and_makes_its_reply_stale() {
        let mut t = Requests::default();
        let id = t.begin(node("a"));
        let other = t.begin(node("b"));
        assert_eq!(t.tracked(), 2);
        assert_eq!(t.forget(&node("a")), vec![id], "the request that was in flight is handed back");
        assert_eq!(t.tracked(), 1, "its generation is gone");
        assert_eq!(t.finish(id), None);
        assert_eq!(t.finish(other), Some(node("b")), "other targets are untouched");
    }

    #[test]
    fn a_target_forgotten_and_begun_again_does_not_accept_the_old_reply() {
        // Generations restart at 1 after a forget: the old request must not be
        // mistaken for the new one.
        let mut t = Requests::default();
        let old = t.begin(node("a"));
        t.forget(&node("a"));
        let new = t.begin(node("a"));
        assert_eq!(t.finish(old), None);
        assert_eq!(t.finish(new), Some(node("a")));
    }

    #[test]
    fn forgetting_by_predicate_drops_every_matching_target() {
        let mut t = Requests::default();
        let (a1, a2, b) = (t.begin(node("a")), t.begin(Target::Run(TabId(1))), t.begin(node("b")));
        let dropped = t.forget_where(|target| matches!(target, Target::Children(_)));
        assert_eq!(dropped, vec![a1, b]);
        assert_eq!(t.tracked(), 1);
        assert_eq!(t.finish(a2), Some(Target::Run(TabId(1))));
    }
}
