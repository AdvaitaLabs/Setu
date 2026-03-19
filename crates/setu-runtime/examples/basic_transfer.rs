//! 演示 Setu Runtime 基本功能的示例程序

use setu_runtime::{
    RuntimeExecutor, ExecutionContext, Transaction, InMemoryStateStore, StateStore,
};
use setu_types::Address;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();
    
    println!("\n=== Setu Runtime 演示 ===\n");
    
    // 1. 创建状态存储
    let mut store = InMemoryStateStore::new();
    
    // 2. 初始化账户和 Coin (devnet genesis addresses)
    let alice = Address::from_hex("0xc0a6c424ac7157ae408398df7e5f4552091a69125d5dfcb7b8c2659029395bdf").unwrap();
    let bob = Address::from_hex("0xeedf1a9c68b3f4a8b1a1032b2b5ad5c4795c026514f8317c7a215e218dccd6cf").unwrap();
    
    println!("👤 Alice: {}", alice);
    println!("👤 Bob: {}", bob);
    println!();
    
    // Alice 有 1000 SETU
    let alice_coin = setu_types::create_coin(alice.clone(), 1000);
    let alice_coin_id = *alice_coin.id();
    println!("💰 创建 Coin for Alice: {} (余额: 1000 SETU)", alice_coin_id);
    store.set_object(alice_coin_id, alice_coin)?;
    
    // Bob 有 500 SETU
    let bob_coin = setu_types::create_coin(bob.clone(), 500);
    let bob_coin_id = *bob_coin.id();
    println!("💰 创建 Coin for Bob: {} (余额: 500 SETU)", bob_coin_id);
    store.set_object(bob_coin_id, bob_coin)?;
    
    // 3. 创建执行器
    let mut executor = RuntimeExecutor::new(store);
    println!("\n✅ Runtime 执行器已创建\n");
    
    // 4. 测试 1: 部分转账 (Alice 转 300 给 Bob)
    println!("=== 测试 1: 部分转账 ===");
    println!("📤 Alice 转账 300 SETU 给 Bob");
    
    let tx1 = Transaction::new_transfer(
        alice.clone(),
        alice_coin_id,
        bob.clone(),
        Some(300), // 部分转账
    );
    
    let ctx = ExecutionContext::new("solver1".to_string(), 1000, false, [0u8; 32]);
    
    let output1 = executor.execute_transaction(&tx1, &ctx)?;
    println!("✅ 交易成功: {}", output1.message.unwrap());
    println!("   - 状态变更: {} 条", output1.state_changes.len());
    println!("   - 创建新对象: {} 个", output1.created_objects.len());
    
    // 验证余额
    let alice_coin = executor.state().get_object(&alice_coin_id)?.unwrap();
    println!("   - Alice 剩余余额: {} SETU", alice_coin.data.balance.value());
    
    let new_bob_coin_id = output1.created_objects[0];
    let new_bob_coin = executor.state().get_object(&new_bob_coin_id)?.unwrap();
    println!("   - Bob 新 Coin 余额: {} SETU", new_bob_coin.data.balance.value());
    println!();
    
    // 5. 测试 2: 完整转账 (Bob 把新收到的 Coin 完全转给 Alice)
    println!("=== 测试 2: 完整转账 ===");
    println!("📤 Bob 完全转账新 Coin 给 Alice");
    
    let tx2 = Transaction::new_transfer(
        bob.clone(),
        new_bob_coin_id,
        alice.clone(),
        None, // 完整转账
    );
    
    let ctx2 = ExecutionContext::new("solver2".to_string(), 2000, false, [1u8; 32]);
    
    let output2 = executor.execute_transaction(&tx2, &ctx2)?;
    println!("✅ 交易成功: {}", output2.message.unwrap());
    println!("   - 状态变更: {} 条", output2.state_changes.len());
    println!("   - 创建新对象: {} 个", output2.created_objects.len());
    
    // 验证所有权变更
    let transferred_coin = executor.state().get_object(&new_bob_coin_id)?.unwrap();
    println!("   - Coin 新所有者: {}", transferred_coin.metadata.owner.as_ref().unwrap());
    println!();
    
    // 6. 测试 3: 查询余额
    println!("=== 测试 3: 查询余额 ===");
    
    let query_tx = Transaction::new_balance_query(alice.clone());
    let ctx3 = ExecutionContext::new("solver3".to_string(), 3000, false, [2u8; 32]);
    
    let output3 = executor.execute_transaction(&query_tx, &ctx3)?;
    println!("✅ 查询成功");
    println!("   - Alice 总余额: {:?}", output3.query_result.unwrap());
    
    // 使用便捷方法查询
    let alice_total = executor.state().get_total_balance(&alice);
    println!("   - Alice 所有 Coin 总额: {} SETU", alice_total);
    
    let bob_total = executor.state().get_total_balance(&bob);
    println!("   - Bob 所有 Coin 总额: {} SETU", bob_total);
    println!();
    
    println!("=== 演示完成 ===\n");
    
    Ok(())
}
