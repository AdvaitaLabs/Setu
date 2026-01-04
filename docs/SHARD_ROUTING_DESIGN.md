# Shard 路由机制设计

## 当前问题

你说得对！我在 `SolverRegistry` 中添加了 `shard_id` 字段，但**没有在路由逻辑中使用它**！

```rust
// 当前的 SolverInfo 有 shard_id
pub struct SolverInfo {
    pub id: String,
    pub shard_id: Option<String>,  // 👈 有这个字段
    pub resources: Vec<String>,
    ...
}

// 但是路由逻辑中没用到！
fn route_by_resource(&self, transfer: &Transfer) -> anyhow::Result<String> {
    if let Some(resource) = transfer.resources.first() {
        if let Some(solver_id) = self.solver_registry.find_by_resource(resource) {
            return Ok(solver_id);
        }
    }
    Ok(self.load_balancer.select_solver()?)
}
```

## 解决方案：添加 Shard 路由

### 方案 1：Transfer 指定 Shard

```rust
// 1. 扩展 Transfer 结构
pub struct Transfer {
    pub id: TransferId,
    pub from: String,
    pub to: String,
    pub amount: i128,
    pub transfer_type: TransferType,
    pub resources: Vec<ResourceKey>,
    pub vlc: Vlc,
    pub power: u64,
    
    // 👇 新增：指定 Shard
    pub shard_id: Option<String>,
}

// 2. 路由逻辑
fn route_by_shard(&self, transfer: &Transfer) -> anyhow::Result<String> {
    if let Some(shard_id) = &transfer.shard_id {
        // 查找该 Shard 中可用的 Solver
        let solvers = self.solver_registry.find_by_shard(shard_id);
        
        if !solvers.is_empty() {
            // 在该 Shard 内做负载均衡
            return self.select_least_loaded_in_shard(&solvers);
        }
    }
    
    // 如果没有指定 Shard，回退到其他策略
    self.route_by_resource(transfer)
}
```

### 方案 2：基于资源自动分片

```rust
// 根据资源哈希自动分配 Shard
fn auto_assign_shard(&self, transfer: &Transfer) -> String {
    // 将资源哈希到 Shard
    let resource_key = transfer.resources.join(":");
    let hash = calculate_hash(&resource_key);
    let shard_count = self.get_shard_count();
    let shard_index = hash % shard_count;
    
    format!("shard-{}", shard_index)
}

fn route_with_auto_shard(&self, transfer: &Transfer) -> anyhow::Result<String> {
    // 1. 自动分配 Shard
    let shard_id = self.auto_assign_shard(transfer);
    
    // 2. 在该 Shard 中选择 Solver
    let solvers = self.solver_registry.find_by_shard(&shard_id);
    
    if !solvers.is_empty() {
        return self.select_least_loaded_in_shard(&solvers);
    }
    
    // 3. 如果该 Shard 没有可用 Solver，回退
    self.load_balancer.select_solver()
}
```

### 方案 3：分层路由（推荐）

```rust
pub struct RouterConfig {
    pub node_id: String,
    pub max_pending_queue_size: usize,
    pub load_balancing_strategy: LoadBalancingStrategy,
    pub quick_check_timeout_ms: u64,
    
    // 👇 新增：路由策略配置
    pub routing_strategy: RoutingStrategy,
}

pub enum RoutingStrategy {
    /// 优先级 1: 手动指定 Solver
    ManualFirst,
    
    /// 优先级 2: Shard 路由
    ShardFirst,
    
    /// 优先级 3: 资源亲和性
    ResourceAffinityFirst,
    
    /// 优先级 4: 负载均衡
    LoadBalanceOnly,
}

// 完整的路由逻辑
fn route_transfer(&self, transfer: &Transfer) -> anyhow::Result<String> {
    match self.config.routing_strategy {
        RoutingStrategy::ManualFirst => {
            // 1. 检查是否手动指定
            if let Some(solver_id) = &transfer.preferred_solver {
                if self.solver_registry.is_available(solver_id) {
                    return Ok(solver_id.clone());
                }
            }
            
            // 2. 检查 Shard
            if let Some(shard_id) = &transfer.shard_id {
                if let Ok(solver_id) = self.route_by_shard_id(shard_id) {
                    return Ok(solver_id);
                }
            }
            
            // 3. 资源亲和性
            if self.config.enable_resource_routing {
                if let Ok(solver_id) = self.route_by_resource(transfer) {
                    return Ok(solver_id);
                }
            }
            
            // 4. 负载均衡
            self.load_balancer.select_solver()
        }
        
        RoutingStrategy::ShardFirst => {
            // Shard 优先...
        }
        
        RoutingStrategy::ResourceAffinityFirst => {
            // 资源优先...
        }
        
        RoutingStrategy::LoadBalanceOnly => {
            // 只用负载均衡
            self.load_balancer.select_solver()
        }
    }
}
```

## 推荐实现

我建议实现一个**灵活的分层路由系统**：

```
优先级 1: 手动指定 (transfer.preferred_solver)
    ↓
优先级 2: Shard 路由 (transfer.shard_id)
    ↓
优先级 3: 资源亲和性 (transfer.resources)
    ↓
优先级 4: 负载均衡 (round-robin/least-loaded)
```

你觉得这个方案如何？需要我实现吗？

