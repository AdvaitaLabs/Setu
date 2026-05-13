# move-compile

Standalone Move compiler tool for Setu smart contracts. Wraps MystenLabs' `move-compiler` from Sui `mainnet-v1.66.2`.

> **Not a workspace member** — this project has its own dependency tree (Sui monorepo) and is built independently from the main Setu workspace.

## Build

```bash
bash tools/move-compile/build.sh
```

First build pulls Sui dependencies (~2min). Subsequent builds are fast.

## Usage

```bash
tools/move-compile/target/release/move-compile \
  <source_files_or_dirs...> \
  --out <output_dir> \
  [--addr name=hex ...] \
  [-- <dependency_dirs...>]
```

### Example: compile the counter contract

```bash
SETU_ROOT=$(pwd)
tools/move-compile/target/release/move-compile \
  examples/move/counter/sources/counter.move \
  --out /tmp/setu-compiled-counter \
  --addr examples=0xcafe --addr setu=0x1 --addr std=0x1 \
  -- setu-framework/sources
```

### Example: compile all sources in a directory

```bash
tools/move-compile/target/release/move-compile \
  examples/move/lightning/sources \
  --out /tmp/setu-compiled-lightning \
  --addr examples=0xcafe --addr setu=0x1 --addr std=0x1 \
  -- setu-framework/sources
```

## Flags

| Flag | Description |
|------|-------------|
| `--out <dir>` | Output directory for `.mv` bytecode files |
| `--addr name=hex` | Named address mapping (repeatable) |
| `--` | Separator; paths after this are dependency source directories |

## Standard Setu address mappings

| Name | Address | Purpose |
|------|---------|---------|
| `setu` | `0x1` | Setu framework modules |
| `std` | `0x1` | Move standard library |
| `examples` | `0xcafe` | Example/user contracts |
