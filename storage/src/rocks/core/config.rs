use std::path::PathBuf;

/// Configuration for RocksDB
#[derive(Debug, Clone)]
pub struct RocksDBConfig {
    /// Path to the RocksDB directory
    pub path: PathBuf,
    
    /// Maximum number of open files
    pub max_open_files: i32,
    
    /// Size of write buffer in bytes (default: 64MB)
    pub write_buffer_size: usize,
    
    /// Maximum number of write buffers (default: 3)
    pub max_write_buffer_number: i32,
    
    /// Target file size for level-1 (default: 64MB)
    pub target_file_size_base: u64,
    
    /// Enable statistics
    pub enable_statistics: bool,
    
    /// Cache size for block cache (default: 512MB)
    pub block_cache_size: usize,
}

impl Default for RocksDBConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("./setu_db"),
            max_open_files: 10000,
            write_buffer_size: 64 * 1024 * 1024,  // 64MB
            max_write_buffer_number: 3,
            target_file_size_base: 64 * 1024 * 1024,  // 64MB
            enable_statistics: true,
            block_cache_size: 512 * 1024 * 1024,  // 512MB
        }
    }
}

impl RocksDBConfig {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            ..Default::default()
        }
    }

    /// Create options for RocksDB from this config
    pub fn to_options(&self) -> rocksdb::Options {
        let mut opts = rocksdb::Options::default();
        
        // Basic options
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        opts.set_max_open_files(self.max_open_files);
        
        // Write buffer settings
        opts.set_write_buffer_size(self.write_buffer_size);
        opts.set_max_write_buffer_number(self.max_write_buffer_number);
        opts.set_target_file_size_base(self.target_file_size_base);
        
        // Compression
        opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
        opts.set_bottommost_compression_type(rocksdb::DBCompressionType::Zstd);
        
        // Performance optimizations
        opts.set_level_compaction_dynamic_level_bytes(true);
        opts.set_bytes_per_sync(1048576);
        
        // Block cache
        let cache = rocksdb::Cache::new_lru_cache(self.block_cache_size);
        let mut block_opts = rocksdb::BlockBasedOptions::default();
        block_opts.set_block_cache(&cache);
        block_opts.set_block_size(16 * 1024);  // 16KB
        block_opts.set_cache_index_and_filter_blocks(true);
        block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);
        opts.set_block_based_table_factory(&block_opts);
        
        // Statistics
        if self.enable_statistics {
            opts.enable_statistics();
            opts.set_statistics_level(rocksdb::statistics::StatsLevel::ExceptDetailedTimers);
        }
        
        opts
    }
}
