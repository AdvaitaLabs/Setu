# Relay Events 规范文档

## 概述

在 Setu 系统中，Relay（中继层/智能合约层）负责监听链上事件，并将这些事件转换为 Transfer 发送给 Router。本文档定义了需要从 Relay 传递到中间层的所有事件类型。

## 架构流程

```
┌─────────────────────────────────────────────────────────────────┐
│                    Relay -> Router 数据流                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  区块链/智能合约                                                  │
│       │                                                         │
│       │ emit Event                                              │
│       ↓                                                         │
│  ┌─────────────┐                                                │
│  │   Relay     │  监听链上事件                                   │
│  │  (中继层)    │  - Validator注册                                │
│  │             │  - Solver注册                                   │
│  └─────────────┘  - SBT铸造/转移                                 │
│       │           - Transfer交易                                │
│       │           - 系统配置变更                                  │
│       │                                                         │
│       │ 转换为 Transfer                                          │
│       ↓                                                         │
│  ┌─────────────┐                                                │
│  │   Router    │  接收并路由                                     │
│  └─────────────┘                                                │
│       │                                                         │
│       ↓                                                         │
│  ┌─────────────┐                                                │
│  │   Solver    │  执行并生成Event                                │
│  └─────────────┘                                                │
│       │                                                         │
│       │ Event (包含执行结果)                                      │
│       ↓                                                         │
│  ┌─────────────┐                                                │
│  │  Validator  │  验证并构建DAG                                  │
│  └─────────────┘                                                │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

## Event 类型定义

### 当前 Event 结构

```rust
pub struct Event {
    pub id: EventId,                           // 事件ID
    pub event_type: EventType,                 // 事件类型
    pub parent_ids: Vec<EventId>,              // 父事件ID（因果依赖）
    pub transfer: Option<Transfer>,            // 关联的Transfer
    pub vlc_snapshot: VLCSnapshot,             // VLC时钟快照
    pub creator: String,                       // 创建者（Solver ID）
    pub status: EventStatus,                   // 事件状态
    pub execution_result: Option<ExecutionResult>, // 执行结果
    pub timestamp: u64,                        // 时间戳
}

pub enum EventType {
    Transfer,    // 转账事件
    System,      // 系统事件
    Genesis,     // 创世事件
}
```

## 需要定义的 Relay Events

基于你的需求，我们需要扩展 `EventType` 和 `Transfer` 来支持以下链上事件：

### 1. Validator 注册事件

**链上事件：**
```solidity
event ValidatorRegistered(
    address indexed validator,
    string nodeId,
    uint256 stake,
    string publicKey,
    string networkEndpoint,
    uint256 timestamp
);
```

**转换为 Transfer：**
```rust
pub struct ValidatorRegistration {
    pub validator_address: String,      // 验证者地址
    pub node_id: String,                // 节点ID
    pub stake_amount: u128,             // 质押金额
    pub public_key: String,             // 公钥（用于签名验证）
    pub network_endpoint: String,       // 网络端点（IP:Port）
    pub commission_rate: u32,           // 佣金率（基点，如500=5%）
    pub metadata: ValidatorMetadata,    // 元数据
}

pub struct ValidatorMetadata {
    pub name: String,                   // 验证者名称
    pub website: String,                // 网站
    pub description: String,            // 描述
    pub logo_url: String,               // Logo URL
}

// Transfer 表示
Transfer {
    id: format!("validator_reg_{}", tx_hash),
    from: validator_address,            // 注册者地址
    to: "system:validator_registry",    // 系统地址
    amount: stake_amount,               // 质押金额
    transfer_type: TransferType::ValidatorRegistration,
    resources: vec![
        format!("validator:{}", validator_address),
        "system:validator_set".to_string(),
    ],
    vlc: vlc_from_block,
    power: stake_amount as u64,         // 质押越多，优先级越高
}
```

### 2. Validator 注销事件

**链上事件：**
```solidity
event ValidatorUnregistered(
    address indexed validator,
    uint256 unstakeAmount,
    uint256 unlockTime,
    uint256 timestamp
);
```

**转换为 Transfer：**
```rust
Transfer {
    id: format!("validator_unreg_{}", tx_hash),
    from: "system:validator_registry",
    to: validator_address,              // 退还质押
    amount: unstake_amount,
    transfer_type: TransferType::ValidatorUnregistration,
    resources: vec![
        format!("validator:{}", validator_address),
        "system:validator_set".to_string(),
    ],
    vlc: vlc_from_block,
    power: 0,
}
```

### 3. Solver 注册事件

**链上事件：**
```solidity
event SolverRegistered(
    address indexed solver,
    string nodeId,
    uint256 stake,
    uint256 capacity,
    string teeAttestation,
    string[] resources,
    uint256 timestamp
);
```

**转换为 Transfer：**
```rust
pub struct SolverRegistration {
    pub solver_address: String,         // Solver地址
    pub node_id: String,                // 节点ID
    pub stake_amount: u128,             // 质押金额
    pub capacity: u32,                  // 处理能力（TPS）
    pub tee_attestation: String,        // TEE证明
    pub resources: Vec<String>,         // 处理的资源类型
    pub shard_id: Option<String>,       // 分片ID
    pub network_endpoint: String,       // 网络端点
}

Transfer {
    id: format!("solver_reg_{}", tx_hash),
    from: solver_address,
    to: "system:solver_registry",
    amount: stake_amount,
    transfer_type: TransferType::SolverRegistration,
    resources: vec![
        format!("solver:{}", solver_address),
        "system:solver_set".to_string(),
    ],
    vlc: vlc_from_block,
    power: capacity as u64,             // 容量越大，优先级越高
}
```

### 4. Solver 注销事件

**链上事件：**
```solidity
event SolverUnregistered(
    address indexed solver,
    uint256 unstakeAmount,
    uint256 unlockTime,
    uint256 timestamp
);
```

**转换为 Transfer：**
```rust
Transfer {
    id: format!("solver_unreg_{}", tx_hash),
    from: "system:solver_registry",
    to: solver_address,
    amount: unstake_amount,
    transfer_type: TransferType::SolverUnregistration,
    resources: vec![
        format!("solver:{}", solver_address),
        "system:solver_set".to_string(),
    ],
    vlc: vlc_from_block,
    power: 0,
}
```

### 5. SBT (Soulbound Token) 铸造事件

**链上事件：**
```solidity
event SBTMinted(
    address indexed to,
    uint256 indexed tokenId,
    string tokenURI,
    bytes32 credentialHash,
    uint256 timestamp
);
```

**转换为 Transfer：**
```rust
pub struct SBTMint {
    pub recipient: String,              // 接收者地址
    pub token_id: u128,                 // Token ID
    pub token_uri: String,              // Token URI（元数据）
    pub credential_hash: String,        // 凭证哈希
    pub sbt_type: SBTType,              // SBT类型
    pub attributes: HashMap<String, String>, // 属性
}

pub enum SBTType {
    Identity,           // 身份SBT
    Reputation,         // 声誉SBT
    Achievement,        // 成就SBT
    Credential,         // 凭证SBT
}

Transfer {
    id: format!("sbt_mint_{}", token_id),
    from: "system:sbt_minter",          // 系统铸造者
    to: recipient,
    amount: 1,                          // SBT数量为1（不可分割）
    transfer_type: TransferType::SBTMint,
    resources: vec![
        format!("sbt:{}", token_id),
        format!("account:{}", recipient),
    ],
    vlc: vlc_from_block,
    power: 100,
}
```

### 6. SBT 更新事件

**链上事件：**
```solidity
event SBTUpdated(
    uint256 indexed tokenId,
    string newTokenURI,
    bytes32 newCredentialHash,
    uint256 timestamp
);
```

**转换为 Transfer：**
```rust
Transfer {
    id: format!("sbt_update_{}", token_id),
    from: current_owner,
    to: current_owner,                  // 自己更新自己
    amount: 0,                          // 无金额变化
    transfer_type: TransferType::SBTUpdate,
    resources: vec![
        format!("sbt:{}", token_id),
    ],
    vlc: vlc_from_block,
    power: 50,
}
```

### 7. SBT 撤销事件

**链上事件：**
```solidity
event SBTRevoked(
    uint256 indexed tokenId,
    address indexed owner,
    string reason,
    uint256 timestamp
);
```

**转换为 Transfer：**
```rust
Transfer {
    id: format!("sbt_revoke_{}", token_id),
    from: owner,
    to: "system:sbt_burner",            // 系统销毁地址
    amount: 1,
    transfer_type: TransferType::SBTRevoke,
    resources: vec![
        format!("sbt:{}", token_id),
        format!("account:{}", owner),
    ],
    vlc: vlc_from_block,
    power: 100,
}
```

### 8. 普通 Flux 转账事件

**链上事件：**
```solidity
event FluxTransfer(
    address indexed from,
    address indexed to,
    uint256 amount,
    bytes data,
    uint256 timestamp
);
```

**转换为 Transfer：**
```rust
Transfer {
    id: format!("flux_transfer_{}", tx_hash),
    from: from_address,
    to: to_address,
    amount: amount,
    transfer_type: TransferType::FluxTransfer,
    resources: vec![
        format!("account:{}", from_address),
        format!("account:{}", to_address),
    ],
    vlc: vlc_from_block,
    power: calculate_power(amount, gas_price),
}
```

### 9. Power 消耗事件

**链上事件：**
```solidity
event PowerConsumed(
    address indexed user,
    uint256 amount,
    string taskId,
    uint256 timestamp
);
```

**转换为 Transfer：**
```rust
Transfer {
    id: format!("power_consume_{}", task_id),
    from: user_address,
    to: "system:power_pool",            // 系统Power池
    amount: amount,
    transfer_type: TransferType::PowerConsume,
    resources: vec![
        format!("account:{}", user_address),
        format!("task:{}", task_id),
    ],
    vlc: vlc_from_block,
    power: amount as u64,
}
```

### 10. Task 提交事件

**链上事件：**
```solidity
event TaskSubmitted(
    address indexed submitter,
    string taskId,
    uint256 reward,
    bytes taskData,
    uint256 deadline,
    uint256 timestamp
);
```

**转换为 Transfer：**
```rust
pub struct TaskSubmission {
    pub task_id: String,
    pub submitter: String,
    pub reward: u128,
    pub task_data: Vec<u8>,
    pub deadline: u64,
    pub required_solvers: u32,
}

Transfer {
    id: format!("task_submit_{}", task_id),
    from: submitter,
    to: "system:task_pool",
    amount: reward,                     // 锁定奖励
    transfer_type: TransferType::TaskSubmit,
    resources: vec![
        format!("task:{}", task_id),
        format!("account:{}", submitter),
    ],
    vlc: vlc_from_block,
    power: reward as u64,
}
```

### 11. 系统配置变更事件

**链上事件：**
```solidity
event SystemConfigUpdated(
    string configKey,
    bytes configValue,
    address indexed updater,
    uint256 timestamp
);
```

**转换为 Transfer：**
```rust
Transfer {
    id: format!("config_update_{}", config_key),
    from: updater,
    to: "system:config",
    amount: 0,
    transfer_type: TransferType::SystemConfigUpdate,
    resources: vec![
        format!("config:{}", config_key),
    ],
    vlc: vlc_from_block,
    power: 1000,                        // 系统配置高优先级
}
```

### 12. Slash 惩罚事件

**链上事件：**
```solidity
event NodeSlashed(
    address indexed node,
    string nodeType,  // "validator" or "solver"
    uint256 slashAmount,
    string reason,
    uint256 timestamp
);
```

**转换为 Transfer：**
```rust
Transfer {
    id: format!("slash_{}", tx_hash),
    from: node_address,
    to: "system:slash_pool",            // 惩罚池
    amount: slash_amount,
    transfer_type: TransferType::Slash,
    resources: vec![
        format!("{}:{}", node_type, node_address),
        "system:slash_pool".to_string(),
    ],
    vlc: vlc_from_block,
    power: 1000,                        // 惩罚事件高优先级
}
```

## 扩展的 TransferType 枚举

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferType {
    // 原有类型
    FluxTransfer,           // Flux转账
    PowerConsume,           // Power消耗
    TaskSubmit,             // 任务提交
    
    // 新增：节点管理
    ValidatorRegistration,  // Validator注册
    ValidatorUnregistration,// Validator注销
    SolverRegistration,     // Solver注册
    SolverUnregistration,   // Solver注销
    
    // 新增：SBT相关
    SBTMint,               // SBT铸造
    SBTUpdate,             // SBT更新
    SBTRevoke,             // SBT撤销
    
    // 新增：系统事件
    SystemConfigUpdate,    // 系统配置更新
    Slash,                 // 惩罚
    Reward,                // 奖励
    
    // 新增：治理
    ProposalCreated,       // 提案创建
    ProposalVoted,         // 提案投票
    ProposalExecuted,      // 提案执行
}
```

## Relay 实现建议

### Relay 结构

```rust
pub struct Relay {
    // 区块链连接
    web3_client: Web3Client,
    
    // 合约地址
    validator_registry_contract: Address,
    solver_registry_contract: Address,
    sbt_contract: Address,
    flux_contract: Address,
    
    // 发送到Router的通道
    transfer_tx: mpsc::UnboundedSender<Transfer>,
    
    // VLC管理
    vlc: VectorClock,
    
    // 事件过滤器
    event_filters: HashMap<String, EventFilter>,
    
    // 最后处理的区块
    last_processed_block: u64,
}

impl Relay {
    /// 启动Relay，监听链上事件
    pub async fn run(&mut self) {
        loop {
            // 1. 获取最新区块
            let latest_block = self.web3_client.get_latest_block().await;
            
            // 2. 处理新区块
            for block_num in self.last_processed_block..=latest_block {
                self.process_block(block_num).await;
            }
            
            // 3. 更新最后处理的区块
            self.last_processed_block = latest_block;
            
            // 4. 等待下一个区块
            tokio::time::sleep(Duration::from_secs(12)).await;
        }
    }
    
    /// 处理单个区块
    async fn process_block(&mut self, block_num: u64) {
        let events = self.web3_client.get_block_events(block_num).await;
        
        for event in events {
            match event.event_type.as_str() {
                "ValidatorRegistered" => {
                    self.handle_validator_registration(event).await;
                }
                "SolverRegistered" => {
                    self.handle_solver_registration(event).await;
                }
                "SBTMinted" => {
                    self.handle_sbt_mint(event).await;
                }
                "FluxTransfer" => {
                    self.handle_flux_transfer(event).await;
                }
                // ... 其他事件
                _ => {}
            }
        }
    }
    
    /// 处理Validator注册事件
    async fn handle_validator_registration(&mut self, event: ChainEvent) {
        let transfer = Transfer {
            id: format!("validator_reg_{}", event.tx_hash),
            from: event.validator_address,
            to: "system:validator_registry".to_string(),
            amount: event.stake_amount,
            transfer_type: TransferType::ValidatorRegistration,
            resources: vec![
                format!("validator:{}", event.validator_address),
                "system:validator_set".to_string(),
            ],
            vlc: self.create_vlc_from_block(event.block_number),
            power: event.stake_amount as u64,
        };
        
        // 发送到Router
        self.transfer_tx.send(transfer).unwrap();
    }
    
    /// 从区块信息创建VLC
    fn create_vlc_from_block(&mut self, block_number: u64) -> Vlc {
        self.vlc.increment("relay");
        
        let mut vlc = Vlc::new();
        vlc.entries.insert("relay".to_string(), self.vlc.get("relay"));
        vlc.entries.insert("block".to_string(), block_number);
        vlc
    }
}
```

## 事件优先级

不同类型的事件有不同的优先级（通过 `power` 字段表示）：

| 事件类型 | 优先级 (power) | 说明 |
|---------|---------------|------|
| SystemConfigUpdate | 1000 | 系统配置最高优先级 |
| Slash | 1000 | 惩罚事件高优先级 |
| ValidatorRegistration | stake_amount | 基于质押金额 |
| SolverRegistration | capacity | 基于处理能力 |
| SBTMint | 100 | SBT铸造中等优先级 |
| FluxTransfer | amount/1000 | 基于转账金额 |
| TaskSubmit | reward | 基于任务奖励 |
| PowerConsume | amount | 基于消耗量 |

## 数据流示例

### 示例1：Validator注册

```
1. 链上：用户调用 ValidatorRegistry.register(nodeId, stake, publicKey)
   ↓
2. 合约：emit ValidatorRegistered(...)
   ↓
3. Relay：监听到事件，转换为Transfer
   Transfer {
       id: "validator_reg_0x123...",
       from: "0xabc...",
       to: "system:validator_registry",
       amount: 10000,
       transfer_type: ValidatorRegistration,
       ...
   }
   ↓
4. Router：接收Transfer，进行Quick Check
   ↓
5. Router：路由到Solver（可能是系统专用Solver）
   ↓
6. Solver：执行注册逻辑
   - 验证质押金额
   - 验证公钥格式
   - 更新Validator集合
   - 生成Event
   ↓
7. Validator：验证Event并加入DAG
   ↓
8. 共识：定期折叠，确认Validator注册
```

### 示例2：SBT铸造

```
1. 链上：SBT合约 mint(to, tokenId, tokenURI, credentialHash)
   ↓
2. 合约：emit SBTMinted(...)
   ↓
3. Relay：转换为Transfer
   Transfer {
       id: "sbt_mint_12345",
       from: "system:sbt_minter",
       to: "0xuser...",
       amount: 1,
       transfer_type: SBTMint,
       ...
   }
   ↓
4. Router -> Solver -> Validator
   ↓
5. Solver执行：
   - 创建SBT对象
   - 关联到用户账户
   - 记录凭证哈希
   ↓
6. 生成Event并确认
```

## 总结

### 需要定义的核心Event类型：

1. **节点管理（4个）**
   - ValidatorRegistration
   - ValidatorUnregistration
   - SolverRegistration
   - SolverUnregistration

2. **SBT相关（3个）**
   - SBTMint
   - SBTUpdate
   - SBTRevoke

3. **资产转移（3个）**
   - FluxTransfer
   - PowerConsume
   - TaskSubmit

4. **系统事件（3个）**
   - SystemConfigUpdate
   - Slash
   - Reward

5. **治理（3个）**
   - ProposalCreated
   - ProposalVoted
   - ProposalExecuted

### 实现要点：

1. **Relay需要实现**：
   - 监听多个合约的事件
   - 将链上事件转换为统一的Transfer格式
   - 维护VLC时钟
   - 处理区块重组

2. **Router需要支持**：
   - 识别不同的TransferType
   - 针对系统事件的特殊路由策略
   - 高优先级事件的快速通道

3. **Solver需要支持**：
   - 不同TransferType的执行逻辑
   - 系统状态的更新（Validator集合、Solver集合等）
   - SBT对象的管理

4. **Validator需要支持**：
   - 验证不同类型事件的合法性
   - 系统事件的特殊验证规则
   - 节点注册/注销的共识确认







