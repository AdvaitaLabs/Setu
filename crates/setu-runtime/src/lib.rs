//! Setu Runtime - Simple State Transition Execution
//! 
//! 这个 crate 提供了一个简化的运行时环境，用于在引入 Move VM 之前
//! 验证 Setu 的核心机制。它实现了基础的状态转换函数，支持：
//! - 转账操作
//! - 余额查询
//! - 对象所有权转移
//! 
//! 未来可以平滑过渡到 Move VM 而不影响其他组件。

pub mod executor;
pub mod state;
pub mod transaction;
pub mod error;

pub use executor::{RuntimeExecutor, ExecutionContext, ExecutionOutput};
pub use state::{StateStore, InMemoryStateStore};
pub use transaction::{Transaction, TransactionType, TransferTx, QueryTx};
pub use error::{RuntimeError, RuntimeResult};
