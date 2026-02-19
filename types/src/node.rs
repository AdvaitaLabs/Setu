use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeRole {
    Validator,
    Solver,
    LightNode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    Initializing,
    Syncing,
    Active,
    Inactive,
    Disconnected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: String,
    pub role: NodeRole,
    pub status: NodeStatus,
    pub address: String,
    pub port: u16,
    pub public_key: Vec<u8>,
    pub stake: u64,
    pub tee_enabled: bool,
}

impl NodeInfo {
    pub fn new_validator(id: String, address: String, port: u16) -> Self {
        Self {
            id,
            role: NodeRole::Validator,
            status: NodeStatus::Initializing,
            address,
            port,
            public_key: Vec::new(),
            stake: 0,
            tee_enabled: false,
        }
    }

    pub fn new_solver(id: String, address: String, port: u16, stake: u64) -> Self {
        Self {
            id,
            role: NodeRole::Solver,
            status: NodeStatus::Initializing,
            address,
            port,
            public_key: Vec::new(),
            stake,
            tee_enabled: false,
        }
    }

    pub fn new_light_node(id: String, address: String, port: u16) -> Self {
        Self {
            id,
            role: NodeRole::LightNode,
            status: NodeStatus::Initializing,
            address,
            port,
            public_key: Vec::new(),
            stake: 0,
            tee_enabled: false,
        }
    }

    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.address, self.port)
    }

    pub fn is_validator(&self) -> bool {
        self.role == NodeRole::Validator
    }

    pub fn is_solver(&self) -> bool {
        self.role == NodeRole::Solver
    }

    pub fn is_active(&self) -> bool {
        self.status == NodeStatus::Active
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorInfo {
    pub node: NodeInfo,
    pub is_leader: bool,
    pub leader_round: u64,
}

impl ValidatorInfo {
    pub fn new(node: NodeInfo, is_leader: bool) -> Self {
        Self {
            node,
            is_leader,
            leader_round: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverInfo {
    pub node: NodeInfo,
    pub processing_capacity: u64,
    pub current_load: u64,
}

impl SolverInfo {
    pub fn new(node: NodeInfo, processing_capacity: u64) -> Self {
        Self {
            node,
            processing_capacity,
            current_load: 0,
        }
    }

    pub fn available_capacity(&self) -> u64 {
        self.processing_capacity.saturating_sub(self.current_load)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TEEAttestation {
    pub node_id: String,
    pub attestation_data: Vec<u8>,
    pub signature: Vec<u8>,
    pub timestamp: u64,
}

impl TEEAttestation {
    pub fn mock(node_id: String) -> Self {
        Self {
            node_id,
            attestation_data: vec![0u8; 32],
            signature: vec![0u8; 64],
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        }
    }

    pub fn verify(&self) -> bool {
        !self.attestation_data.is_empty() && !self.signature.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_info_creation() {
        let validator = NodeInfo::new_validator("v1".to_string(), "127.0.0.1".to_string(), 8000);
        assert!(validator.is_validator());
        assert_eq!(validator.endpoint(), "127.0.0.1:8000");
    }

    #[test]
    fn test_solver_capacity() {
        let node = NodeInfo::new_solver("s1".to_string(), "127.0.0.1".to_string(), 9000, 1000);
        let mut solver = SolverInfo::new(node, 50);
        assert_eq!(solver.available_capacity(), 50);

        solver.current_load = 30;
        assert_eq!(solver.available_capacity(), 20);
    }
}
