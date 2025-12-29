use setu_types::{
    Anchor, ConsensusConfig, ConsensusFrame, EventId, Vote,
};
use crate::dag::Dag;
use crate::vlc::VLC;
use std::collections::HashMap;

#[derive(Debug)]
pub struct DagFolder {
    config: ConsensusConfig,
    last_anchor: Option<Anchor>,
    anchor_depth: u64,
    last_fold_vlc: u64,
}

impl DagFolder {
    pub fn new(config: ConsensusConfig) -> Self {
        Self {
            config,
            last_anchor: None,
            anchor_depth: 0,
            last_fold_vlc: 0,
        }
    }

    pub fn should_fold(&self, current_vlc: &VLC) -> bool {
        let delta = current_vlc.logical_time().saturating_sub(self.last_fold_vlc);
        delta >= self.config.vlc_delta_threshold
    }

    pub fn fold(&mut self, dag: &Dag, vlc: &VLC, state_root: String) -> Option<Anchor> {
        if !self.should_fold(vlc) {
            return None;
        }

        let from_depth = self.anchor_depth;
        let to_depth = dag.max_depth();

        let events = dag.get_events_in_range(from_depth, to_depth);
        
        if events.len() < self.config.min_events_per_cf {
            return None;
        }

        let event_ids: Vec<EventId> = events
            .iter()
            .take(self.config.max_events_per_cf)
            .map(|e| e.id.clone())
            .collect();

        let anchor = Anchor::new(
            event_ids,
            vlc.snapshot(),
            state_root,
            self.last_anchor.as_ref().map(|a| a.id.clone()),
            to_depth,
        );

        self.last_anchor = Some(anchor.clone());
        self.anchor_depth = to_depth + 1;
        self.last_fold_vlc = vlc.logical_time();

        Some(anchor)
    }

    pub fn last_anchor(&self) -> Option<&Anchor> {
        self.last_anchor.as_ref()
    }

    pub fn anchor_depth(&self) -> u64 {
        self.anchor_depth
    }
}

#[derive(Debug)]
pub struct ConsensusManager {
    config: ConsensusConfig,
    folder: DagFolder,
    pending_cfs: HashMap<String, ConsensusFrame>,
    finalized_cfs: Vec<ConsensusFrame>,
    local_validator_id: String,
}

impl ConsensusManager {
    pub fn new(config: ConsensusConfig, validator_id: String) -> Self {
        Self {
            config: config.clone(),
            folder: DagFolder::new(config),
            pending_cfs: HashMap::new(),
            finalized_cfs: Vec::new(),
            local_validator_id: validator_id,
        }
    }

    pub fn try_create_cf(
        &mut self,
        dag: &Dag,
        vlc: &VLC,
        state_root: String,
    ) -> Option<ConsensusFrame> {
        let anchor = self.folder.fold(dag, vlc, state_root)?;
        let cf = ConsensusFrame::new(anchor, self.local_validator_id.clone());
        self.pending_cfs.insert(cf.id.clone(), cf.clone());
        Some(cf)
    }

    pub fn receive_cf(&mut self, cf: ConsensusFrame) {
        if !self.pending_cfs.contains_key(&cf.id) {
            self.pending_cfs.insert(cf.id.clone(), cf);
        }
    }

    pub fn vote_for_cf(&mut self, cf_id: &str, approve: bool) -> Option<Vote> {
        let cf = self.pending_cfs.get_mut(cf_id)?;
        
        if cf.votes.contains_key(&self.local_validator_id) {
            return None;
        }

        let vote = Vote::new(self.local_validator_id.clone(), cf_id.to_string(), approve);
        cf.add_vote(vote.clone());
        
        Some(vote)
    }

    pub fn receive_vote(&mut self, vote: Vote) -> bool {
        let cf_id = vote.cf_id.clone();
        if let Some(cf) = self.pending_cfs.get_mut(&cf_id) {
            cf.add_vote(vote);
        } else {
            return false;
        }
        self.check_finalization(&cf_id)
    }

    fn check_finalization(&mut self, cf_id: &str) -> bool {
        let should_finalize = {
            let cf = match self.pending_cfs.get(cf_id) {
                Some(cf) => cf,
                None => return false,
            };
            cf.check_quorum(self.config.validator_count)
        };

        if should_finalize {
            if let Some(mut cf) = self.pending_cfs.remove(cf_id) {
                cf.finalize();
                self.finalized_cfs.push(cf);
                return true;
            }
        }

        false
    }

    pub fn get_pending_cf(&self, cf_id: &str) -> Option<&ConsensusFrame> {
        self.pending_cfs.get(cf_id)
    }

    pub fn finalized_count(&self) -> usize {
        self.finalized_cfs.len()
    }

    pub fn last_finalized_cf(&self) -> Option<&ConsensusFrame> {
        self.finalized_cfs.last()
    }

    pub fn should_fold(&self, vlc: &VLC) -> bool {
        self.folder.should_fold(vlc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use setu_types::{Event, EventType};

    fn create_vlc(node_id: &str, time: u64) -> VLC {
        let mut vlc = VLC::new(node_id.to_string());
        for _ in 0..time {
            vlc.tick();
        }
        vlc
    }

    fn setup_dag_with_events(count: usize) -> (Dag, VLC) {
        let mut dag = Dag::new();
        let mut vlc = VLC::new("node1".to_string());

        let genesis = Event::genesis("node1".to_string(), vlc.snapshot());
        let mut last_id = dag.add_event(genesis).unwrap();

        for _ in 1..count {
            vlc.tick();
            let event = Event::new(
                EventType::Transfer,
                vec![last_id.clone()],
                vlc.snapshot(),
                "node1".to_string(),
            );
            last_id = dag.add_event(event).unwrap();
        }

        (dag, vlc)
    }

    #[test]
    fn test_folder_should_fold() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 10,
            ..Default::default()
        };
        let folder = DagFolder::new(config);
        
        let vlc = create_vlc("node1", 5);
        assert!(!folder.should_fold(&vlc));

        let vlc = create_vlc("node1", 10);
        assert!(folder.should_fold(&vlc));
    }

    #[test]
    fn test_consensus_manager_create_cf() {
        let config = ConsensusConfig {
            vlc_delta_threshold: 5,
            min_events_per_cf: 1,
            ..Default::default()
        };
        let mut manager = ConsensusManager::new(config, "validator1".to_string());
        let (dag, vlc) = setup_dag_with_events(10);

        let cf = manager.try_create_cf(&dag, &vlc, "state_root".to_string());
        assert!(cf.is_some());
    }
}
