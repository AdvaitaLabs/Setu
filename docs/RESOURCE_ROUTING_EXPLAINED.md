# Router 资源路由机制详解

## 1. 什么是资源（Resource）？

在 Setu 中，**资源（Resource）** 是指 Transfer 涉及的账户或对象。

### 当前实现：

```rust
pub struct Transfer {
    pub id: TransferId,
    pub from: String,        // 发送方
    pub to: String,          // 接收方
    pub amount: i128,
    pub transfer_type: TransferType,
    pub resources: Vec<ResourceKey>,  // 👈 资源列表
    pub vlc: Vlc,
    pub power: u64,
}
```

### 资源的含义：

```rust
// 示例 1: Alice 转账给 Bob
Transfer {
    id: "tx-001",
    from: "alice",
    to: "bob",
    amount: 1000,
    resources: vec!["alice", "bob"],  // 涉及两个账户资源
    ...
}

// 示例 2: Alice 消耗 Power
Transfer {
    id: "tx-002",
    from: "alice",
    to: "system",
    amount: 100,
    resources: vec!["alice"],  // 只涉及 alice 账户
    ...
}
```

## 2. 资源路由的工作原理

### 当前实现（简化版）：

```rust
fn route_by_resource(&self, transfer: &Transfer) -> anyhow::Result<String> {
    // 使用第一个资源作为路由键
    if let Some(resource) = transfer.resources.first() {
        // 查找处理该资源的 Solver
        if let Some(solver_id) = self.solver_registry.find_by_resource(resource) {
            return Ok(solver_id);
        }
    }
    
    // 如果没找到，回退到负载均衡
    Ok(self.load_balancer.select_solver()?)
}
```

### 问题：**只使用第一个资源，不够智能！**

## 3. 改进方案：支持自定义路由规则

### 方案 A：基于资源哈希的一致性路由

```rust
fn route_by_resource_hash(&self, transfer: &Transfer) -> anyhow::Result<String> {
    // 将所有资源排序后哈希
    let mut resources = transfer.resources.clone();
    resources.sort();
    let resource_key = resources.join(":");
    
    // 使用一致性哈希选择 Solver
    let hash = calculate_hash(&resource_key);
    let solver_id = self.solver_registry.find_by_hash(hash)?;
    
    Ok(solver_id)
}
```

### 方案 B：基于资源亲和性的智能路由

```rust
fn route_by_resource_affinity(&self, transfer: &Transfer) -> anyhow::Result<String> {
    // 1. 查找所有能处理这些资源的 Solver
    let mut candidate_solvers = Vec::new();
    
    for resource in &transfer.resources {
        if let Some(solver_ids) = self.solver_registry.find_all_by_resource(resource) {
            candidate_solvers.extend(solver_ids);
        }
    }
    
    // 2. 如果有多个候选，选择负载最低的
    if !candidate_solvers.is_empty() {
        let solver_id = self.select_least_loaded(&candidate_solvers)?;
        return Ok(solver_id);
    }
    
    // 3. 如果没有候选，回退到负载均衡
    Ok(self.load_balancer.select_solver()?)
}
```

### 方案 C：支持手动指定 Solver（你想要的功能）

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
    
    // 👇 新增：手动指定 Solver
    pub preferred_solver: Option<String>,  // 优先使用的 Solver
    pub allowed_solvers: Option<Vec<String>>,  // 允许的 Solver 列表
}

// 2. 路由逻辑
fn route_with_preference(&self, transfer: &Transfer) -> anyhow::Result<String> {
    // 优先级 1: 手动指定的 Solver
    if let Some(solver_id) = &transfer.preferred_solver {
        if self.solver_registry.is_available(solver_id) {
            return Ok(solver_id.clone());
        }
    }
    
    // 优先级 2: 允许的 Solver 列表中选择
    if let Some(allowed) = &transfer.allowed_solvers {
        let available: Vec<_> = allowed.iter()
            .filter(|id| self.solver_registry.is_available(id))
            .collect();
        
        if !available.is_empty() {
            return Ok(self.select_least_loaded(&available)?);
        }
    }
    
    // 优先级 3: 资源亲和性路由
    if self.config.enable_resource_routing {
        return self.route_by_resource_affinity(transfer);
    }
    
    // 优先级 4: 负载均衡
    Ok(self.load_balancer.select_solver()?)
}
```

## 4. 推荐的完整方案

我建议实现一个**分层路由策略**：

```rust
pub enum RoutingStrategy {
    /// 手动指定（最高优先级）
    Manual { solver_id: String },
    
    /// 资源亲和性（根据账户/对象）
    ResourceAffinity,
    
    /// Shard 路由（根据分片）
    ShardBased { shard_id: String },
    
    /// 一致性哈希
    ConsistentHash,
    
    /// 负载均衡（最低优先级）
    LoadBalance,
}
```

你觉得哪个方案最适合你的需求？我可以帮你实现！

