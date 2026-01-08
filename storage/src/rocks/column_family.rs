/// Column families used in Setu storage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColumnFamily {
    Objects,
    Coins,
    CoinsByOwner,
    Profiles,
    ProfileByAddress,
    Credentials,
    CredentialsByHolder,
    CredentialsByIssuer,
    RelationGraphs,
    GraphsByOwner,
    Events,
    Anchors,
    Checkpoints,
    // Merkle tree storage
    MerkleNodes,
    MerkleRoots,
}

impl ColumnFamily {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Objects => "objects",
            Self::Coins => "coins",
            Self::CoinsByOwner => "coins_by_owner",
            Self::Profiles => "profiles",
            Self::ProfileByAddress => "profile_by_address",
            Self::Credentials => "credentials",
            Self::CredentialsByHolder => "credentials_by_holder",
            Self::CredentialsByIssuer => "credentials_by_issuer",
            Self::RelationGraphs => "relation_graphs",
            Self::GraphsByOwner => "graphs_by_owner",
            Self::Events => "events",
            Self::Anchors => "anchors",
            Self::Checkpoints => "checkpoints",
            Self::MerkleNodes => "merkle_nodes",
            Self::MerkleRoots => "merkle_roots",
        }
    }
    
    pub fn all() -> Vec<Self> {
        vec![
            Self::Objects,
            Self::Coins,
            Self::CoinsByOwner,
            Self::Profiles,
            Self::ProfileByAddress,
            Self::Credentials,
            Self::CredentialsByHolder,
            Self::CredentialsByIssuer,
            Self::RelationGraphs,
            Self::GraphsByOwner,
            Self::Events,
            Self::Anchors,
            Self::Checkpoints,
            Self::MerkleNodes,
            Self::MerkleRoots,
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
                    Self::CoinsByOwner | Self::GraphsByOwner | Self::ProfileByAddress |
                    Self::CredentialsByHolder | Self::CredentialsByIssuer => {
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
