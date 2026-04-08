use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use setu_runtime::{
    compile_package_to_disassembly, ExecutionContext, RuntimeExecutor, SetuMerkleStateStore,
    StateStore, SuiVmArg, SuiVmStoredObject, SuiVmStoredValue, Transaction,
};
use setu_types::{Address, ObjectId};
use tempfile::TempDir;

const CONTRACT: &str = r#"module persistent_counter_pkg::counter {
    public struct Counter has key, store {
        id: UID,
        value: u64,
    }

    entry fun increment(counter: &mut Counter) {
        let current = counter.value;
        counter.value = current + 1;
    }
}"#;

struct PersistentCounterExample {
    _db_dir: TempDir,
    db_path: PathBuf,
    owner: Address,
    counter_id: ObjectId,
    disassembly: String,
}

fn setup_state() -> Result<PersistentCounterExample> {
    let pkg = create_temp_package_with_contract()?;
    println!("Created package: {}", pkg.display());

    let disassembly = compile_package_to_disassembly(&pkg, "counter")
        .context("Failed to compile and disassemble counter contract")?;
    println!("Compiled + disassembled module: counter");

    let db_dir = TempDir::new().context("Failed to create temp directory")?;
    let db_path = db_dir.path().join("setu_merkle_db");
    println!("Setu storage path: {}", db_path.display());

    let owner = Address::from_str_id("alice");
    let counter_id = ObjectId::new([0x31; 32]);

    let mut state =
        SetuMerkleStateStore::open_root(&db_path).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    state
        .set_vm_object(
            counter_id,
            SuiVmStoredObject::new_owned(
                counter_id,
                "Counter",
                owner,
                std::collections::BTreeMap::from([(
                    "value".to_string(),
                    SuiVmStoredValue::U64(41),
                )]),
            ),
        )
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let anchor_id = state
        .commit_pending()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    println!(
        "Seeded ROOT subnet counter {} with value 41 at anchor {}",
        counter_id, anchor_id
    );
    println!("Initial state root: 0x{}", to_hex(&state.state_root()));

    Ok(PersistentCounterExample {
        _db_dir: db_dir,
        db_path,
        owner,
        counter_id,
        disassembly,
    })
}

fn execute_scenario(example: &PersistentCounterExample) -> Result<()> {
    let state = SetuMerkleStateStore::open_root(&example.db_path)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let before_root = state.state_root();
    let mut executor = RuntimeExecutor::new(state);

    execute_program_tx(
        &mut executor,
        &example.owner,
        &example.disassembly,
        "increment",
        vec![SuiVmArg::ObjectId(example.counter_id)],
        10,
        "persistent_objects",
    )?;

    let commit_anchor = executor
        .state_mut()
        .commit_pending()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let counter = executor
        .state()
        .get_vm_object(&example.counter_id)?
        .context("counter missing after increment")?;
    let value = counter
        .get_u64_field("value")
        .context("counter missing 'value' field after increment")?;
    if value != 42 {
        bail!("expected counter value 42 after increment, got {}", value);
    }

    let after_root = executor.state().state_root();
    if before_root == after_root {
        bail!("expected state root to change after increment");
    }

    println!(
        "Increment executed: counter {} is now {} at anchor {}",
        example.counter_id, value, commit_anchor
    );
    println!("Updated state root: 0x{}", to_hex(&after_root));

    Ok(())
}

fn assert_state(example: &PersistentCounterExample) -> Result<()> {
    let reopened = SetuMerkleStateStore::open_root(&example.db_path)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let counter = reopened
        .get_vm_object(&example.counter_id)?
        .context("counter missing after reopening persisted Setu state")?;
    let value = counter
        .get_u64_field("value")
        .context("counter missing 'value' field after reopening")?;
    if value != 42 {
        bail!("expected reopened counter value 42, got {}", value);
    }
    if reopened.get_object_bytes(&example.counter_id).is_none() {
        bail!("MerkleStateProvider should return raw object bytes for the counter");
    }

    println!(
        "Reopened state: counter {} recovered with value {}",
        example.counter_id, value
    );
    println!("Recovered state root: 0x{}", to_hex(&reopened.state_root()));
    println!("\nPersistent counter example completed.");

    Ok(())
}

fn main() -> Result<()> {
    let example = setup_state()?;
    execute_scenario(&example)?;
    assert_state(&example)
}

fn execute_program_tx<S: StateStore>(
    executor: &mut RuntimeExecutor<S>,
    sender: &Address,
    disassembly: &str,
    function_name: &str,
    args: Vec<SuiVmArg>,
    timestamp: u64,
    executor_id: &str,
) -> Result<()> {
    let tx = Transaction::new_program_deterministic(
        *sender,
        disassembly.to_owned(),
        function_name,
        args,
        timestamp,
    );
    let ctx = ExecutionContext {
        executor_id: executor_id.to_string(),
        timestamp,
        in_tee: false,
    };

    executor
        .execute_transaction(&tx, &ctx)
        .with_context(|| format!("Failed to execute '{}' via RuntimeExecutor", function_name))?;

    Ok(())
}

fn create_temp_package_with_contract() -> Result<PathBuf> {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let package_prefix = format!("persistent_counter_pkg_{}", ts);
    let root = std::env::temp_dir().join(format!("persistent_counter_example_{}", ts));
    fs::create_dir_all(&root)?;

    let status = Command::new("sui")
        .arg("move")
        .arg("new")
        .arg(&package_prefix)
        .current_dir(&root)
        .status()
        .context("Failed to execute `sui move new`")?;
    if !status.success() {
        bail!("`sui move new` failed with status {}", status);
    }

    let pkg = root.join(&package_prefix);
    let src = pkg.join("sources");
    let default_module = src.join(format!("{}.move", package_prefix));
    if default_module.exists() {
        fs::remove_file(default_module)?;
    }

    let contract = CONTRACT.replace(
        "persistent_counter_pkg::counter",
        &format!("{}::counter", package_prefix),
    );
    fs::write(src.join("counter.move"), contract)?;

    Ok(pkg)
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
