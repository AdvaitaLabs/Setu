#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use setu_runtime::{ExecutionContext, RuntimeExecutor, StateStore, SuiVmArg, Transaction};
use setu_types::{deterministic_coin_id, Address, CoinData, Object, ObjectId};
use tempfile::TempDir;

pub struct ExampleState<S: StateStore> {
    pub executor: RuntimeExecutor<S>,
    pub persistent_dir: Option<TempDir>,
    pub persistent_path: Option<PathBuf>,
}

impl<S: StateStore> ExampleState<S> {
    pub fn new(executor: RuntimeExecutor<S>) -> Self {
        Self {
            executor,
            persistent_dir: None,
            persistent_path: None,
        }
    }

    pub fn with_persistent_dir(
        executor: RuntimeExecutor<S>,
        persistent_dir: TempDir,
        persistent_path: PathBuf,
    ) -> Self {
        Self {
            executor,
            persistent_dir: Some(persistent_dir),
            persistent_path: Some(persistent_path),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExampleArgs {
    pub addresses: std::collections::BTreeMap<String, Address>,
    pub object_ids: std::collections::BTreeMap<String, ObjectId>,
}

impl ExampleArgs {
    pub fn new(_executor_id: impl Into<String>) -> Self {
        Self::default()
    }

    pub fn insert_address(&mut self, key: impl Into<String>, value: Address) {
        self.addresses.insert(key.into(), value);
    }

    pub fn insert_object_id(&mut self, key: impl Into<String>, value: ObjectId) {
        self.object_ids.insert(key.into(), value);
    }

    pub fn address(&self, key: &str) -> Result<Address> {
        self.addresses
            .get(key)
            .copied()
            .with_context(|| format!("missing address '{}'", key))
    }

    pub fn object_id(&self, key: &str) -> Result<ObjectId> {
        self.object_ids
            .get(key)
            .copied()
            .with_context(|| format!("missing object id '{}'", key))
    }
}

#[derive(Debug, Clone)]
pub struct ProgramCallSpec {
    pub sender: Address,
    pub disassembly: String,
    pub function_name: String,
    pub args: Vec<SuiVmArg>,
    pub timestamp: u64,
    pub executor_id: String,
}

pub fn execute_program_tx<S: StateStore>(
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

pub fn execute_program_scenario<S: StateStore>(
    mut state: ExampleState<S>,
    calls: &[ProgramCallSpec],
) -> Result<ExampleState<S>> {
    for call in calls {
        execute_program_tx(
            &mut state.executor,
            &call.sender,
            &call.disassembly,
            &call.function_name,
            call.args.clone(),
            call.timestamp,
            &call.executor_id,
        )?;
    }

    state
        .executor
        .state_mut()
        .commit_pending()
        .context("Failed to commit scenario state")?;

    Ok(state)
}

#[allow(dead_code)]
pub fn expect_coin_balance<S: StateStore>(
    state: &S,
    owner: &Address,
    coin_type: &str,
    expected: u64,
    label: &str,
) -> Result<Object<CoinData>> {
    let coin = state
        .get_object(&deterministic_coin_id(owner, coin_type))?
        .with_context(|| format!("{} coin missing", label))?;

    if coin.data.balance.value() != expected {
        bail!(
            "expected {} balance {}, got {}",
            label,
            expected,
            coin.data.balance.value()
        );
    }

    Ok(coin)
}

#[allow(dead_code)]
pub fn create_temp_package_with_contract(
    package_prefix: &str,
    module_filename: &str,
    contract: &str,
) -> Result<PathBuf> {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let root = std::env::temp_dir().join(format!("{}_{}", package_prefix, ts));
    fs::create_dir_all(&root)?;

    let status = Command::new("sui")
        .arg("move")
        .arg("new")
        .arg(package_prefix)
        .current_dir(&root)
        .status()
        .context("Failed to execute `sui move new`")?;
    if !status.success() {
        bail!("`sui move new` failed with status {}", status);
    }

    let pkg = root.join(package_prefix);
    let src = pkg.join("sources");
    let default_module = src.join(format!("{}.move", package_prefix));
    if default_module.exists() {
        fs::remove_file(default_module)?;
    }
    fs::write(src.join(module_filename), contract)?;

    Ok(pkg)
}
