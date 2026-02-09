/// Column families used in Setu storage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColumnFamily {
    Objects,
    Coins,
    CoinsByOwner,
    /// Index: (owner, coin_type) -> Vec<ObjectId>
    /// Enables efficient lookup of coins by owner and type for multi-subnet scenarios
    CoinsByOwnerAndType,
    Profiles,
    ProfileByAddress,
    Credentials,
    CredentialsByHolder,
    CredentialsByIssuer,
    RelationGraphs,
    GraphsByOwner,
    // User relation network storage
    UserRelationNetworks,
    UserRelationNetworkByUser,
    // User subnet activity storage
    UserSubnetActivities,
    UserSubnetActivitiesByUser,
    Events,
    Anchors,
    Checkpoints,
    // Merkle tree storage
    MerkleNodes,
    MerkleRoots,
    // ConsensusFrame storage
    ConsensusFrames,
}

impl ColumnFamily {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Objects => "objects",
            Self::Coins => "coins",
            Self::CoinsByOwner => "coins_by_owner",
            Self::CoinsByOwnerAndType => "coins_by_owner_and_type",
            Self::Profiles => "profiles",
            Self::ProfileByAddress => "profile_by_address",
            Self::Credentials => "credentials",
            Self::CredentialsByHolder => "credentials_by_holder",
            Self::CredentialsByIssuer => "credentials_by_issuer",
            Self::RelationGraphs => "relation_graphs",
            Self::GraphsByOwner => "graphs_by_owner",
            Self::UserRelationNetworks => "user_relation_networks",
            Self::UserRelationNetworkByUser => "user_relation_network_by_user",
            Self::UserSubnetActivities => "user_subnet_activities",
            Self::UserSubnetActivitiesByUser => "user_subnet_activities_by_user",
            Self::Events => "events",
            Self::Anchors => "anchors",
            Self::Checkpoints => "checkpoints",
            Self::MerkleNodes => "merkle_nodes",
            Self::MerkleRoots => "merkle_roots",
            Self::ConsensusFrames => "consensus_frames",
        }
    }
    
    pub fn all() -> Vec<Self> {
        vec![
            Self::Objects,
            Self::Coins,
            Self::CoinsByOwner,
            Self::CoinsByOwnerAndType,
            Self::Profiles,
            Self::ProfileByAddress,
            Self::Credentials,
            Self::CredentialsByHolder,
            Self::CredentialsByIssuer,
            Self::RelationGraphs,
            Self::GraphsByOwner,
            Self::UserRelationNetworks,
            Self::UserRelationNetworkByUser,
            Self::UserSubnetActivities,
            Self::UserSubnetActivitiesByUser,
            Self::Events,
            Self::Anchors,
            Self::Checkpoints,
            Self::MerkleNodes,
            Self::MerkleRoots,
            Self::ConsensusFrames,
        ]
    }
    
    pub fn descriptors() -> Vec<rocksdb::ColumnFamilyDescriptor> {
        Self::all()
            .into_iter()
            .map(|cf| {
                let mut opts = rocksdb::Options::default();
                match cf {
                    Self::Objects => {
                        opts.set_write_buffer_size(128 * 1024 * 1024);
                        opts.set_max_write_buffer_number(4);
                    }
                    Self::Coins | Self::Profiles | Self::Credentials | Self::RelationGraphs => {
                        opts.set_write_buffer_size(64 * 1024 * 1024);
                        opts.set_max_write_buffer_number(3);
                    }
                    Self::CoinsByOwner | Self::CoinsByOwnerAndType | Self::GraphsByOwner | Self::ProfileByAddress |
                    Self::CredentialsByHolder | Self::CredentialsByIssuer => {
                        opts.set_write_buffer_size(32 * 1024 * 1024);
                        opts.set_compression_type(rocksdb::DBCompressionType::Zstd);
                    }
                    Self::UserRelationNetworks | Self::UserSubnetActivities => {
                        opts.set_write_buffer_size(64 * 1024 * 1024);
                        opts.set_max_write_buffer_number(3);
                    }
                    Self::UserRelationNetworkByUser | Self::UserSubnetActivitiesByUser => {
                        opts.set_write_buffer_size(32 * 1024 * 1024);
                        opts.set_compression_type(rocksdb::DBCompressionType::Zstd);
                    }
                    Self::Events | Self::Anchors => {
                        opts.set_write_buffer_size(64 * 1024 * 1024);
                        opts.set_max_write_buffer_number(6);
                    }
                    Self::Checkpoints => {
                        opts.set_write_buffer_size(16 * 1024 * 1024);
                    }
                    Self::MerkleNodes => {
                        // Merkle nodes: high read/write, benefit from larger cache
                        opts.set_write_buffer_size(64 * 1024 * 1024);
                        opts.set_max_write_buffer_number(4);
                        opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
                    }
                    Self::MerkleRoots => {
                        // Merkle roots: smaller, historical data
                        opts.set_write_buffer_size(16 * 1024 * 1024);
                        opts.set_compression_type(rocksdb::DBCompressionType::Zstd);
                    }
                    Self::ConsensusFrames => {
                        // Consensus frames: moderate size, frequent read/write during consensus
                        opts.set_write_buffer_size(32 * 1024 * 1024);
                        opts.set_max_write_buffer_number(4);
                        opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
                    }
                }
                rocksdb::ColumnFamilyDescriptor::new(cf.name(), opts)
            })
            .collect()
    }
}

impl std::fmt::Display for ColumnFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}
