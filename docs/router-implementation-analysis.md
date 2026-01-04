# Router 路由层实现详解

## 1. Router 架构概述

Router 是 Setu 系统中的关键路由层，负责接收来自 Relay 的 Transfer 意图，进行快速验证，并将其路由到合适的 Solver 节点进行执行。

### 1.1 核心职责

```
┌─────────────────────────────────────────────────────────────┐
│                    Router 工作流程                           │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  1. 接收 Transfer Intent (来自 Relay/用户)                   │
│           ↓                                                 │
│  2. Quick Check (快速验证)                                   │
│           ↓                                                 │
│  3. Pending Queue (待处理队列)                               │
│           ↓                                                 │
│  4. Load Balancing / Resource Routing (负载均衡/资源路由)     │
│           ↓                                                 │
│  5. Send to Solver (发送给 Solver)                          │
│           ↓                                                 │
│  6. Update Statistics (更新统计信息)                         │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## 2. 核心模块详解

### 2.1 Router 主结构 (`lib.rs`)

```rust
pub struct Router {
    config: RouterConfig,
    
    // 接收来自客户端/Relay的transfer
    transfer_rx: mpsc::UnboundedReceiver<Transfer>,
    
    // 发送给多个solver的通道
    solver_channels: Arc<parking_lot::RwLock<Vec<mpsc::UnboundedSender<Transfer>>>>,
    
    // 快速检查器
    quick_checker: QuickChecker,
    
    // 负载均衡器
    load_balancer: LoadBalancer,
    
    // 待处理队列
    pending_queue: PendingQueue,
    
    // Solver注册表
    solver_registry: Arc<SolverRegistry>,
    
    // 统计信息
    stats: Arc<parking_lot::RwLock<RouterStats>>,
}
```

**关键配置项：**

```rust
pub struct RouterConfig {
    pub node_id: String,                              // Router节点ID
    pub max_pending_queue_size: usize,                // 最大待处理队列大小 (默认10000)
    pub load_balancing_strategy: LoadBalancingStrategy, // 负载均衡策略
    pub quick_check_timeout_ms: u64,                  // 快速检查超时 (默认100ms)
    pub enable_resource_routing: bool,                // 是否启用资源路由
}
```

### 2.2 Quick Check 快速验证 (`quick_check.rs`)

快速验证是Router的第一道防线，用于过滤掉明显无效的transfer。

**验证项目：**

1. **Transfer ID 非空检查**
   ```rust
   if transfer.id.is_empty() {
       return Err(QuickCheckError::EmptyTransferId);
   }
   ```

2. **发送者验证**
   ```rust
   if transfer.from.is_empty() {
       return Err(QuickCheckError::InvalidSender);
   }
   ```

3. **接收者验证**
   ```rust
   if transfer.to.is_empty() {
       return Err(QuickCheckError::InvalidRecipient);
   }
   ```

4. **金额验证**
   ```rust
   if transfer.amount <= 0 {
       return Err(QuickCheckError::InvalidAmount);
   }
   ```

5. **资源列表验证**
   ```rust
   if transfer.resources.is_empty() {
       return Err(QuickCheckError::EmptyResources);
   }
   ```

6. **VLC验证**
   ```rust
   if transfer.vlc.entries.is_empty() {
       return Err(QuickCheckError::InvalidVLC);
   }
   ```

7. **发送者和接收者不能相同**
   ```rust
   if transfer.from == transfer.to {
       return Err(QuickCheckError::InvalidRecipient);
   }
   ```

### 2.3 Pending Queue 待处理队列 (`pending_queue.rs`)

待处理队列用于暂存通过快速验证但尚未路由的transfer。

**特性：**

- **FIFO队列**：先进先出
- **快速查找**：通过HashMap索引实现O(1)查找
- **容量限制**：防止内存溢出
- **优先级支持**：基于power字段计算优先级

```rust
pub struct PendingQueue {
    max_size: usize,                           // 最大容量
    queue: VecDeque<PendingTransfer>,          // FIFO队列
    index: HashMap<String, usize>,             // transfer_id -> position映射
}

pub struct PendingTransfer {
    pub transfer: Transfer,
    pub enqueued_at: u64,                      // 入队时间戳
    pub priority: u32,                         // 优先级（基于power）
}
```

**关键操作：**

- `enqueue()`: 入队，检查容量和重复
- `dequeue()`: 按ID出队
- `dequeue_next()`: FIFO出队
- `peek_next()`: 查看队首但不移除

### 2.4 Load Balancer 负载均衡 (`load_balancer.rs`)

负载均衡器负责在多个Solver之间分配transfer。

**支持的策略：**

```rust
pub enum LoadBalancingStrategy {
    RoundRobin,          // 轮询（默认）
    Random,              // 随机
    LeastLoaded,         // 最少负载
    WeightedCapacity,    // 按容量加权
}
```

**实现细节：**

1. **RoundRobin（轮询）**
   ```rust
   let index = self.round_robin_counter.fetch_add(1, Ordering::Relaxed) % solvers.len();
   Ok(solvers[index].clone())
   ```

2. **Random（随机）**
   ```rust
   let mut rng = rand::thread_rng();
   let index = rng.gen_range(0..solvers.len());
   Ok(solvers[index].clone())
   ```

3. **LeastLoaded（最少负载）**
   - 查询每个Solver的当前负载
   - 选择负载最低的Solver

4. **WeightedCapacity（加权）**
   - 根据Solver的容量进行加权选择
   - 容量大的Solver获得更多请求

### 2.5 Solver Registry Solver注册表 (`solver_registry.rs`)

Solver注册表维护所有可用Solver的信息和状态。

**Solver信息：**

```rust
pub struct SolverInfo {
    pub id: String,                    // Solver ID
    pub status: SolverStatus,          // 状态
    pub capacity: u32,                 // 最大容量（TPS）
    pub current_load: u32,             // 当前负载
    pub total_processed: u64,          // 总处理数
    pub shard_id: Option<String>,      // 分片ID（可选）
    pub resources: Vec<String>,        // 处理的资源列表
    pub last_heartbeat: u64,           // 最后心跳时间
}

pub enum SolverStatus {
    Active,      // 活跃，接受请求
    Inactive,    // 不活跃
    Overloaded,  // 过载
    Failed,      // 失败
}
```

**关键功能：**

1. **注册Solver**
   ```rust
   pub fn register(&self, solver_info: SolverInfo)
   ```

2. **资源亲和性路由**
   ```rust
   pub fn find_by_resource(&self, resource: &str) -> Option<String>
   ```
   - 根据资源找到处理该资源的Solver
   - 选择负载最低的Solver

3. **分片路由**
   ```rust
   pub fn find_by_shard(&self, shard_id: &str) -> Vec<String>
   ```

4. **负载管理**
   ```rust
   pub fn increment_load(&self, solver_id: &str)  // 增加负载
   pub fn decrement_load(&self, solver_id: &str)  // 减少负载
   ```

5. **健康检查**
   ```rust
   pub fn heartbeat(&self, solver_id: &str)       // 更新心跳
   pub fn remove_stale(&self) -> Vec<String>      // 移除过期Solver
   ```

## 3. Router 工作流程详解

### 3.1 主循环

```rust
pub async fn run(mut self) {
    while let Some(transfer) = self.transfer_rx.recv().await {
        self.update_stats_received();
        
        match self.process_transfer(transfer).await {
            Ok(()) => self.update_stats_routed(),
            Err(e) => self.update_stats_rejected(),
        }
    }
}
```

### 3.2 Transfer处理流程

```rust
async fn process_transfer(&mut self, transfer: Transfer) -> anyhow::Result<()> {
    // Step 1: 快速检查
    self.quick_checker.check(&transfer).await?;
    
    // Step 2: 加入待处理队列
    self.pending_queue.enqueue(transfer.clone())?;
    
    // Step 3: 选择Solver
    let solver_id = if self.config.enable_resource_routing {
        // 资源路由：根据transfer的资源选择Solver
        self.route_by_resource(&transfer)?
    } else {
        // 负载均衡：使用配置的策略选择Solver
        self.load_balancer.select_solver()?
    };
    
    // Step 4: 发送给Solver
    self.send_to_solver(&solver_id, transfer.clone()).await?;
    
    // Step 5: 从待处理队列移除
    self.pending_queue.dequeue(&transfer.id)?;
    
    Ok(())
}
```

### 3.3 资源路由策略

```rust
fn route_by_resource(&self, transfer: &Transfer) -> anyhow::Result<String> {
    // 使用第一个资源作为路由键
    if let Some(resource) = transfer.resources.first() {
        // 查找处理该资源的Solver
        if let Some(solver_id) = self.solver_registry.find_by_resource(resource) {
            return Ok(solver_id);
        }
    }
    
    // 回退到负载均衡
    Ok(self.load_balancer.select_solver()?)
}
```

### 3.4 发送到Solver

```rust
async fn send_to_solver(&self, solver_id: &str, transfer: Transfer) -> anyhow::Result<()> {
    // 1. 获取Solver信息
    let solver_info = self.solver_registry.get(solver_id)?;
    
    // 2. 检查Solver状态
    if solver_info.status != SolverStatus::Active {
        return Err(anyhow!("Solver is not active"));
    }
    
    // 3. 获取Solver通道
    let channels = self.solver_channels.read();
    let solver_index = self.solver_registry.get_index(solver_id)?;
    let solver_tx = &channels[solver_index];
    
    // 4. 发送transfer
    solver_tx.send(transfer)?;
    
    // 5. 更新Solver负载
    self.solver_registry.increment_load(solver_id);
    
    Ok(())
}
```

## 4. Solver注册流程

### 4.1 基本注册

```rust
pub fn register_solver(
    &self,
    solver_id: String,
    solver_tx: mpsc::UnboundedSender<Transfer>,
    capacity: u32,
) {
    // 1. 创建SolverInfo
    let solver_info = SolverInfo::new(solver_id.clone(), capacity);
    
    // 2. 注册到registry
    self.solver_registry.register(solver_info);
    
    // 3. 添加通道
    let mut channels = self.solver_channels.write();
    channels.push(solver_tx);
    
    // 4. 添加到负载均衡器
    self.load_balancer.add_solver(solver_id);
    
    // 5. 更新统计
    self.update_active_solvers_count();
}
```

### 4.2 带亲和性的注册

```rust
pub fn register_solver_with_affinity(
    &self,
    solver_id: String,
    solver_tx: mpsc::UnboundedSender<Transfer>,
    capacity: u32,
    shard_id: Option<String>,      // 分片ID
    resources: Vec<String>,         // 资源列表
) {
    let mut solver_info = SolverInfo::new(solver_id.clone(), capacity);
    solver_info.shard_id = shard_id;
    solver_info.resources = resources;
    
    // 注册流程同上...
}
```

## 5. 统计信息

```rust
pub struct RouterStats {
    pub total_received: u64,        // 总接收数
    pub total_routed: u64,          // 总路由数
    pub total_rejected: u64,        // 总拒绝数
    pub pending_queue_size: usize,  // 待处理队列大小
    pub active_solvers: usize,      // 活跃Solver数量
}
```

## 6. 使用示例

```rust
// 创建Router配置
let router_config = RouterConfig {
    node_id: "router-1".to_string(),
    max_pending_queue_size: 10000,
    load_balancing_strategy: LoadBalancingStrategy::RoundRobin,
    quick_check_timeout_ms: 100,
    enable_resource_routing: true,
};

// 创建通道
let (transfer_tx, transfer_rx) = mpsc::unbounded_channel::<Transfer>();
let (solver_tx, solver_rx) = mpsc::unbounded_channel::<Transfer>();

// 创建Router
let router = Router::new(router_config, transfer_rx);

// 注册Solver
router.register_solver("solver-1".to_string(), solver_tx, 100);

// 启动Router
tokio::spawn(async move {
    router.run().await;
});

// 发送transfer
transfer_tx.send(transfer).unwrap();
```

## 7. 性能优化

### 7.1 并发控制

- 使用`parking_lot::RwLock`提供高性能读写锁
- 使用`Arc`实现零拷贝共享

### 7.2 快速路径

- Quick Check在100ms内完成
- 使用HashMap实现O(1)查找
- 无锁的原子计数器（RoundRobin）

### 7.3 容量管理

- 待处理队列限制防止内存溢出
- Solver过载检测和自动降级
- 过期Solver自动清理（30s无心跳）

## 8. 未来扩展

### 8.1 网络层

当前使用mpsc通道（单进程），未来将替换为：
- RPC (gRPC/Anemo)
- 跨网络的Solver发现
- 动态Solver注册/注销

### 8.2 高级路由

- 基于地理位置的路由
- 基于延迟的智能路由
- 多级路由（Router -> Router -> Solver）

### 8.3 监控和可观测性

- Prometheus metrics
- 分布式追踪（OpenTelemetry）
- 实时性能仪表板







