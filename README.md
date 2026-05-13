<div align="center">

<pre>
 ███████╗███████╗████████╗██╗   ██╗
 ██╔════╝██╔════╝╚══██╔══╝██║   ██║
 ███████╗█████╗     ██║   ██║   ██║
 ╚════██║██╔══╝     ██║   ██║   ██║
 ███████║███████╗   ██║   ╚██████╔╝
 ╚══════╝╚══════╝   ╚═╝    ╚═════╝
</pre>

<h3>The ledger that ticks when AI agents work — not when miners do.</h3>

<p>
A causally-driven distributed ledger purpose-built for AI agent economies.<br/>
Replace <em>physical time + miners</em> with <em>verifiable work + TEE attestation</em>.
</p>

<p>
<a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-1.75+-FF6B35?style=for-the-badge&logo=rust&logoColor=white" alt="Rust" /></a>
<a href="https://github.com/AdvaitaLabs/Setu/stargazers"><img src="https://img.shields.io/github/stars/AdvaitaLabs/Setu?style=for-the-badge&logo=github&color=FFD93D" alt="Stars" /></a>
<a href="https://discord.gg/hetu"><img src="https://img.shields.io/badge/discord-join-5865F2?style=for-the-badge&logo=discord&logoColor=white" alt="Discord" /></a>
<a href="https://twitter.com/Hetu_Protocol"><img src="https://img.shields.io/badge/twitter-follow-1DA1F2?style=for-the-badge&logo=twitter&logoColor=white" alt="Twitter" /></a>
</p>

<p>
<img src="https://img.shields.io/badge/⚡_200K--300K-TPS-00C896?style=flat-square" />
<img src="https://img.shields.io/badge/⏱_50--100-ms_finality-00C896?style=flat-square" />
<img src="https://img.shields.io/badge/🛡_TEE-secured-00C896?style=flat-square" />
<img src="https://img.shields.io/badge/❌-no_miners-FF4757?style=flat-square" />
<img src="https://img.shields.io/badge/❌-no_gas-FF4757?style=flat-square" />
</p>

<p>
<a href="#-quick-demo"><b>⚡ Quick Demo</b></a> &nbsp;·&nbsp;
<a href="#-what-you-can-build"><b>🛠 Use Cases</b></a> &nbsp;·&nbsp;
<a href="#-how-setu-works"><b>🏗 Architecture</b></a> &nbsp;·&nbsp;
<a href="#-get-started"><b>📦 Get Started</b></a> &nbsp;·&nbsp;
<a href="https://docs.hetu.org"><b>📚 Docs</b></a> &nbsp;·&nbsp;
<a href="https://discord.gg/hetu"><b>💬 Discord</b></a>
</p>

<br/>

<img src="https://raw.githubusercontent.com/AdvaitaLabs/Setu/main/docs/assets/setu-demo.gif" alt="Setu Demo" width="700" onerror="this.style.display='none'" />


</div>

<br/>

---

## ⚡ What is Setu

> [!TIP]
> **30-second pitch:** If you're shipping AI agents that act thousands of times per second, you need a substrate that records, proves, and settles their work — without the cost, latency, or trust assumptions of traditional blockchains. Setu is that substrate.

If you're building AI agents that need to:

<table>
<tr>
<td width="50%">

#### 🔐 Prove work was done
With cryptographic TEE attestation

</td>
<td width="50%">

#### 💸 Get paid by other agents
In milliseconds, no gas wars

</td>
</tr>
<tr>
<td width="50%">

#### 🏆 Build verifiable reputation
On-chain, queryable by anyone

</td>
<td width="50%">

#### 🔗 Coordinate without trust
No central server, no oracle

</td>
</tr>
</table>

...Setu gives you the substrate.

### 🆚 Setu vs Ethereum

Think of Setu as **"Ethereum for AI agents"** — except:

<table>
<thead>
<tr>
<th align="left">&nbsp;</th>
<th align="center">🟧 Ethereum</th>
<th align="center">🟦 Setu</th>
</tr>
</thead>
<tbody>
<tr>
<td><b>Block trigger</b></td>
<td align="center">⏰ Physical time (12s)</td>
<td align="center">⚡ <b>Causal work events</b></td>
</tr>
<tr>
<td><b>Execution</b></td>
<td align="center">📖 Public VM</td>
<td align="center">🛡 <b>TEE enclave (Nitro)</b></td>
</tr>
<tr>
<td><b>Ordering</b></td>
<td align="center">🕒 Global clock</td>
<td align="center">🔢 <b>Vector Logical Clock</b></td>
</tr>
<tr>
<td><b>Throughput</b></td>
<td align="center"><code>~30 TPS</code></td>
<td align="center"><code><b>200K-300K TPS</b></code></td>
</tr>
<tr>
<td><b>Finality</b></td>
<td align="center"><code>~12 seconds</code></td>
<td align="center"><code><b>50-100ms</b></code></td>
</tr>
<tr>
<td><b>Gas auctions</b></td>
<td align="center">💸 Yes</td>
<td align="center">✨ <b>None</b></td>
</tr>
</tbody>
</table>

---

## 🚀 Quick Demo

> [!NOTE]
> **60 seconds. Zero config. First transaction recorded.**

```bash
# 1️⃣  Clone and start a local Setu network
git clone https://github.com/AdvaitaLabs/Setu.git && cd Setu
docker-compose -f docker/docker-compose.multi-validator.yml up -d

# 2️⃣  Submit a transfer between two agents
curl -X POST http://localhost:8080/api/v1/transfer \
  -H "Content-Type: application/json" \
  -d '{"from": "agent_alice", "to": "agent_bob", "amount": 100}'

# 3️⃣  Query balance — finalized in ~50ms
curl http://localhost:8080/api/v1/balance/agent_bob
```

> [!TIP]
> ✅ Work recorded &nbsp;·&nbsp; ✅ TEE-attested &nbsp;·&nbsp; ✅ Finalized in 50ms

<sub>Want more? See the <a href="#api-reference">full API reference</a> or jump to <a href="#path-2--run-your-own-node">Run Your Own Node</a>.</sub>

---

## 🛠 What You Can Build

<table>
<tr>
<td width="50%" valign="top">

### 🤖 Agent Work Tracker

Record every action an AI agent takes, with cryptographic proof of execution.

**Build "git for AI agents"** — append-only, verifiable, queryable. Any auditor can reconstruct what an agent did and prove nothing was tampered with.

<sub>🎯 Best for: AI audit trails, compliance, agent debugging</sub>

</td>
<td width="50%" valign="top">

### 🏆 Agent Reputation System

Let agents accumulate verified work history.

Other agents (or humans) can query: *"Has this agent reliably completed similar tasks?"* — get a cryptographically signed answer in milliseconds. **Trust networks without central authorities.**

<sub>🎯 Best for: Agent marketplaces, AI staffing, trust networks</sub>

</td>
</tr>
<tr>
<td width="50%" valign="top">

### 💸 Agent-to-Agent Payments

Settle micropayments between agents in **~50ms**.

No gas auctions, no human approvals, no block-time waits. At 200K+ TPS, **per-second pricing becomes realistic.**

<sub>🎯 Best for: AI service marketplaces, agent economies</sub>

</td>
<td width="50%" valign="top">

### 🔗 Multi-Agent Coordination

When multiple agents collaborate, Setu records **causal dependencies** via VLC.

Reconstruct *"who did what, when, and what depended on what."* Critical for debugging multi-agent systems and assigning responsibility.

<sub>🎯 Best for: Multi-agent frameworks, swarm intelligence</sub>

</td>
</tr>
<tr>
<td width="50%" valign="top">

### 🛡 Verifiable AI Inference

Run inference inside a TEE (Nitro Enclave). Setu records the attestation.

Verify: *"This output came from this model, this hardware, this input."* **Useful for AI auditing, compliance, adversarial-proof inference.**

<sub>🎯 Best for: Regulated AI, medical AI, financial AI</sub>

</td>
<td width="50%" valign="top">

### 🧪 ...And More

Setu is a substrate. If your use case involves agents acting at scale with verifiable work, reach out.

[📝 Open an issue](https://github.com/AdvaitaLabs/Setu/issues/new) &nbsp;·&nbsp;
[💬 Join Discord](https://discord.gg/hetu)

<sub>🎯 Best for: Your next idea</sub>

</td>
</tr>
</table>

---

## 🤔 Why Setu

<details>
<summary>🟧 <b>Why not just put agent records on Ethereum / Solana?</b></summary>

<br>

AI agents act **thousands of times per second**. At Ethereum's 30 TPS and gas costs, a single agent doing routine work would saturate the chain and pay $1,000+/day in gas. Setu's 200K-300K TPS and no-gas design makes per-action recording realistic.

</details>

<details>
<summary>🟦 <b>Why not just use a database?</b></summary>

<br>

Because the whole point is **trustless verifiability**. If an agent's reputation lives in a database, the database operator controls it. Setu lets the agent control its own work history, with the network providing tamper-evidence.

</details>

<details>
<summary>🟪 <b>Why not use a centralized AI execution service?</b></summary>

<br>

Centralized services can lie about what they ran. Setu uses TEE attestation (AWS Nitro) so anyone can later verify: *"this exact model ran on this exact hardware with this exact input."* You don't trust us. You verify.

</details>

<details>
<summary>🟩 <b>Is this a Layer 1? Layer 2?</b></summary>

<br>

Neither, exactly. Setu is a **causally-driven distributed ledger** — closer to a DAG-based consensus network than a traditional blockchain. It can settle to Bitcoin or Ethereum if you need finality on a public chain, but it doesn't *need* one to function.

</details>

<details>
<summary>🟨 <b>Is Setu production-ready?</b></summary>

<br>

Setu is in active development. The core consensus, VLC, TEE execution, and HTTP API are working and benchmarked. Production mainnet is on the [roadmap](#-roadmap). Use the current code for testnets, research, and pre-production prototypes.

</details>

---

## 🏗 How Setu Works

### Architecture

```
╔══════════════════════════════════════════════════════════════════════════════╗
║                              Setu Network                                    ║
╠═════════════════════════════════╦════════════════════════════════════════════╣
║         Validator Nodes         ║           Solver Nodes                     ║
║                                 ║                                            ║
║  ┌──────────────────────────┐   ║   ┌──────────────────────────┐             ║
║  │    ConsensusEngine       │   ║   │      TeeExecutor         │             ║
║  │  ┌────────┐ ┌─────────┐  │   ║   │                          │             ║
║  │  │  DAG   │ │   VLC   │  │   ║   │  ┌────────────────────┐  │             ║
║  │  └────────┘ └─────────┘  │   ║   │  │   EnclaveRuntime   │  │             ║
║  │  ┌────────────────────┐  │   ║   │  │  (Mock / Nitro)    │  │             ║
║  │  │  ValidatorSet      │  │   ║   │  └────────────────────┘  │             ║
║  │  │  (Leader Election) │  │   ║   │                          │             ║
║  │  └────────────────────┘  │   ║   └──────────────────────────┘             ║
║  │  ┌────────────────────┐  │   ║                                            ║
║  │  │   AnchorBuilder    │  │   ║   ┌──────────────────────────┐             ║
║  │  │  (Merkle Roots)    │  │   ║   │   SolverNetworkClient    │             ║
║  │  └────────────────────┘  │   ║   └──────────────────────────┘             ║
║  └──────────────────────────┘   ║                                            ║
║                                 ║                                            ║
║  ┌──────────────────────────┐   ║                                            ║
║  │  GlobalStateManager      │   ║                                            ║
║  │  (Sparse Merkle Trees)   │   ║                                            ║
║  └──────────────────────────┘   ║                                            ║
╠═════════════════════════════════╩════════════════════════════════════════════╣
║                          P2P Network (Anemo / QUIC)                          ║
╚══════════════════════════════════════════════════════════════════════════════╝
```

### 🔑 The 5 Core Innovations

<table>
<thead>
<tr>
<th>🟢 Innovation</th>
<th>What it does</th>
<th>Why it matters</th>
</tr>
</thead>
<tbody>
<tr>
<td><b>🌐 DAG-Based Consensus</b></td>
<td>DAG-BFT with leader rotation</td>
<td>High throughput without sacrificing finality</td>
</tr>
<tr>
<td><b>⏱ Vector Logical Clock</b></td>
<td>Causal ordering of distributed events</td>
<td>Agents act in parallel; ordering still verifiable</td>
</tr>
<tr>
<td><b>🛡 TEE Execution</b></td>
<td>Trusted Execution (AWS Nitro)</td>
<td>Verifiable AI inference, private data, attested output</td>
</tr>
<tr>
<td><b>🔀 Subnet Architecture</b> <sub><i>in development</i></sub></td>
<td>Multi-subnet support</td>
<td>Horizontal scaling — each workload its own subnet</td>
</tr>
<tr>
<td><b>🌳 Merkle State Commitment</b></td>
<td>Binary + Sparse Merkle Trees</td>
<td>Light clients verify state without full sync</td>
</tr>
</tbody>
</table>

### 🔄 Consensus Flow

```
  ┌─────────┐    ┌───────────┐    ┌──────────┐    ┌──────────┐    ┌─────────┐
  │  Event  │ ─► │    TEE    │ ─► │ Verifier │ ─► │   DAG    │ ─► │  State  │
  │ Submit  │    │ Execution │    │  Adds to │    │ Folding  │    │ Commit  │
  └─────────┘    └───────────┘    │   DAG    │    │ (Anchor) │    └─────────┘
                                  └──────────┘    └──────────┘
       ▲                                                ▲
       │                                                │
   Agent / Client                                ConsensusFrame
                                              Quorum 2/3+1 voting
```

**Key Concepts:**

- 🟢 **Event** — Atomic state change with TEE attestation
- 🟡 **Anchor** — Checkpoint containing events + Merkle roots
- 🔵 **ConsensusFrame (CF)** — Voting unit for finalization
- 🟣 **VLC** — Vector Logical Clock for causal ordering

<sub>Deeper dive: <a href="docs/architecture.md">docs/architecture.md</a></sub>

---

## 📦 Get Started

### 🟢 Path 1 · Use Setu in your app

> [!TIP]
> Most users start here. You don't need to run your own node.

```bash
# Submit a transfer
curl -X POST http://your-validator:8080/api/v1/transfer \
  -H "Content-Type: application/json" \
  -d '{"from": "alice", "to": "bob", "amount": 100}'

# Query balance
curl http://your-validator:8080/api/v1/balance/bob

# Query an object
curl http://your-validator:8080/api/v1/object/<object_id>

# List recent events
curl http://your-validator:8080/api/v1/events
```

> [!NOTE]
> 💡 Python / TypeScript / Go SDKs are on the [roadmap](#-roadmap). For now, any HTTP client works.

### 🟦 Path 2 · Run Your Own Node

<details>
<summary><b>📋 Prerequisites</b></summary>

<br>

- **Rust** 1.75+ (2021 edition)
- **RocksDB** for persistent storage
- **Docker** for containerized deployment (optional)

</details>

<details>
<summary><b>🔨 Build from source</b></summary>

<br>

```bash
git clone https://github.com/AdvaitaLabs/Setu.git
cd Setu
cargo build --release
cargo test --all
```

</details>

<details>
<summary><b>⚙️ Run a Validator</b></summary>

<br>

```bash
export VALIDATOR_ID=validator-1
export VALIDATOR_HTTP_PORT=8080
export VALIDATOR_P2P_PORT=9000
export VALIDATOR_DB_PATH=/tmp/setu/validator

./target/release/setu-validator
```

</details>

<details>
<summary><b>🧮 Run a Solver</b></summary>

<br>

```bash
export SOLVER_ID=solver-1
export SOLVER_PORT=9001
export VALIDATOR_ADDRESS=127.0.0.1
export VALIDATOR_HTTP_PORT=8080

./target/release/setu-solver
```

</details>

<details>
<summary><b>💻 Submit Transactions via CLI</b></summary>

<br>

```bash
./target/release/setu balance --address <ADDRESS>
./target/release/setu transfer --from <FROM> --to <TO> --amount 100
```

</details>

<details>
<summary><b>🐳 Docker Deployment (multi-validator)</b></summary>

<br>

```bash
cd docker
./scripts/build.sh
docker-compose -f docker-compose.multi-validator.yml up -d
docker-compose logs -f
```

</details>

---

## 📁 Project Structure

<details>
<summary><b>Click to expand the full project tree</b></summary>

<br>

```
Setu/
├── consensus/              # DAG-based consensus implementation
│   ├── dag.rs             # DAG data structure
│   ├── engine.rs          # Main consensus engine
│   ├── anchor_builder.rs  # Anchor creation with Merkle trees
│   ├── folder.rs          # ConsensusManager for CF management
│   └── vlc.rs             # VLC integration
│
├── types/                  # Core type definitions
│   ├── event.rs           # Event, EventId, EventStatus
│   ├── consensus.rs       # Anchor, ConsensusFrame, Vote
│   ├── object.rs          # Object model (Coin, Profile, etc.)
│   └── merkle.rs          # Merkle tree types
│
├── storage/                # Storage layer
│   ├── memory/            # In-memory implementations (DashMap)
│   ├── rocks/             # RocksDB persistent storage
│   └── state/             # GlobalStateManager, StateProvider
│
├── crates/
│   ├── setu-vlc/          # VLC Hybrid Logical Clock library
│   ├── setu-merkle/       # Merkle trees (Binary + Sparse)
│   ├── setu-keys/         # Cryptographic key management
│   ├── setu-enclave/      # TEE abstraction (Mock + Nitro)
│   ├── setu-network-anemo/# Anemo-based P2P network
│   ├── setu-transport/    # HTTP/WS/gRPC transport layer
│   ├── setu-protocol/     # Protocol message definitions
│   └── setu-core/         # Shared core utilities
│
├── setu-validator/         # Validator node binary
├── setu-solver/            # Solver node binary
├── setu-cli/               # CLI tool
├── setu-rpc/               # RPC layer
├── setu-benchmark/         # TPS benchmark tool
│
├── api/                    # HTTP API layer
├── docker/                 # Docker deployment configs
├── scripts/                # Deployment & test scripts
└── docs/                   # Design documents
```

</details>

---

## ⚙️ Configuration

<details>
<summary><b>🟢 Validator environment variables</b></summary>

<br>

| Variable | Default | Description |
|----------|---------|-------------|
| `VALIDATOR_ID` | `validator-1` | Unique validator identifier |
| `VALIDATOR_HTTP_PORT` | `8080` | HTTP API port |
| `VALIDATOR_P2P_PORT` | `9000` | P2P network port |
| `VALIDATOR_DB_PATH` | (memory) | RocksDB path for persistence |
| `VALIDATOR_KEY_FILE` | — | Path to keypair file |

</details>

<details>
<summary><b>🟦 Solver environment variables</b></summary>

<br>

| Variable | Default | Description |
|----------|---------|-------------|
| `SOLVER_ID` | `solver-{uuid}` | Unique solver identifier |
| `SOLVER_PORT` | `9001` | Solver listen port |
| `SOLVER_CAPACITY` | `100` | Max concurrent tasks |
| `VALIDATOR_ADDRESS` | `127.0.0.1` | Validator address |
| `AUTO_REGISTER` | `true` | Auto-register on startup |

</details>

<details>
<summary><b>🟣 Consensus configuration</b></summary>

<br>

| Parameter | Default | Description |
|-----------|---------|-------------|
| `vlc_delta_threshold` | `10` | VLC delta to trigger folding |
| `min_events_per_cf` | `1` | Minimum events per ConsensusFrame |
| `max_events_per_cf` | `1000` | Maximum events per ConsensusFrame |
| `vote_timeout_ms` | `5000` | Voting timeout in milliseconds |

</details>

---

## 🔌 API Reference

### HTTP Endpoints

<table>
<thead>
<tr>
<th>Method</th>
<th>Endpoint</th>
<th>Description</th>
</tr>
</thead>
<tbody>
<tr>
<td><img src="https://img.shields.io/badge/GET-61AFFE?style=flat-square" /></td>
<td><code>/health</code></td>
<td>Health check</td>
</tr>
<tr>
<td><img src="https://img.shields.io/badge/POST-49CC90?style=flat-square" /></td>
<td><code>/api/v1/transfer</code></td>
<td>Submit transfer between agents</td>
</tr>
<tr>
<td><img src="https://img.shields.io/badge/GET-61AFFE?style=flat-square" /></td>
<td><code>/api/v1/balance/{address}</code></td>
<td>Query balance for an address</td>
</tr>
<tr>
<td><img src="https://img.shields.io/badge/GET-61AFFE?style=flat-square" /></td>
<td><code>/api/v1/object/{id}</code></td>
<td>Query an object by ID</td>
</tr>
<tr>
<td><img src="https://img.shields.io/badge/GET-61AFFE?style=flat-square" /></td>
<td><code>/api/v1/events</code></td>
<td>List recent events</td>
</tr>
<tr>
<td><img src="https://img.shields.io/badge/POST-49CC90?style=flat-square" /></td>
<td><code>/api/v1/register/solver</code></td>
<td>Register a new solver</td>
</tr>
</tbody>
</table>

### RPC Services

- 🟢 **ConsensusService** — Event submission, CF proposal, voting
- 🟦 **SyncService** — Event/CF synchronization between validators
- 🟣 **DiscoveryService** — Peer discovery and management

<sub>Full API docs: <a href="docs/api.md">docs/api.md</a></sub>

---

## 📊 Performance

<div align="center">

<table>
<thead>
<tr>
<th>Metric</th>
<th>Value</th>
<th>Notes</th>
</tr>
</thead>
<tbody>
<tr>
<td>⚡ <b>TPS</b></td>
<td align="center"><img src="https://img.shields.io/badge/200K--300K-00C896?style=for-the-badge" /></td>
<td>DAG-BFT consensus</td>
</tr>
<tr>
<td>⏱ <b>Latency</b></td>
<td align="center"><img src="https://img.shields.io/badge/50--100_ms-00C896?style=for-the-badge" /></td>
<td>End-to-end confirmation</td>
</tr>
<tr>
<td>🛡 <b>Validators</b></td>
<td align="center"><img src="https://img.shields.io/badge/7-3D5AFE?style=for-the-badge" /></td>
<td>BFT consensus quorum</td>
</tr>
<tr>
<td>🧮 <b>Solvers</b></td>
<td align="center"><img src="https://img.shields.io/badge/21-3D5AFE?style=for-the-badge" /></td>
<td>Horizontal scaling</td>
</tr>
</tbody>
</table>

</div>

### Run Benchmarks Yourself

```bash
# Simple TPS test
python scripts/tps_test_simple.py

# Full benchmark
cargo bench --package setu-benchmark
```

---

## 🗺 Roadmap

<table>
<tr>
<td width="33%" valign="top">

### 🟢 Now `v0.x`

✅ DAG-BFT consensus<br/>
✅ VLC causal ordering<br/>
✅ TEE execution (Nitro + Mock)<br/>
✅ HTTP API + CLI<br/>
✅ Multi-validator Docker<br/>
✅ 200K+ TPS benchmark

</td>
<td width="33%" valign="top">

### 🟡 Next `Q3 2026`

🔨 Subnet architecture<br/>
🔨 Python SDK<br/>
🔨 TypeScript SDK<br/>
🔨 LangChain integration<br/>
🔨 Public testnet launch

</td>
<td width="33%" valign="top">

### 🔵 Later `Q4 2026+`

📋 MCP server integration<br/>
📋 Cross-chain bridges<br/>
📋 Reputation primitives<br/>
📋 Production mainnet<br/>
📋 ZK proof integration

</td>
</tr>
</table>

> [!TIP]
> 💬 Want to influence the roadmap? **[Open an issue](https://github.com/AdvaitaLabs/Setu/issues)** or join our **[Discord](https://discord.gg/hetu)**.

---

## 🤝 Community

<table align="center">
<tr>
<td align="center" width="25%">
<a href="https://discord.gg/hetu">
<img src="https://img.shields.io/badge/Discord-Join-5865F2?style=for-the-badge&logo=discord&logoColor=white" alt="Discord" /><br/>
<sub><b>Questions, help, dev chat</b></sub>
</a>
</td>
<td align="center" width="25%">
<a href="https://twitter.com/Hetu_Protocol">
<img src="https://img.shields.io/badge/Twitter-Follow-1DA1F2?style=for-the-badge&logo=twitter&logoColor=white" alt="Twitter" /><br/>
<sub><b>Updates & announcements</b></sub>
</a>
</td>
<td align="center" width="25%">
<a href="https://hetu.org">
<img src="https://img.shields.io/badge/Website-hetu.org-000?style=for-the-badge" alt="Website" /><br/>
<sub><b>Full ecosystem site</b></sub>
</a>
</td>
<td align="center" width="25%">
<a href="https://docs.hetu.org">
<img src="https://img.shields.io/badge/Docs-Read-3D5AFE?style=for-the-badge&logo=readthedocs&logoColor=white" alt="Docs" /><br/>
<sub><b>Deep technical docs</b></sub>
</a>
</td>
</tr>
</table>

### Get Involved

| Action | How |
|--------|-----|
| ⭐ **Star this repo** | If Setu solves a problem you have |
| 🐛 **Open an issue** | Bug reports, feature requests |
| 🔧 **Submit a PR** | See [Contributing Guidelines](CONTRIBUTING.md) — start with [`good-first-issue`](https://github.com/AdvaitaLabs/Setu/labels/good-first-issue) |
| 💬 **Join the discussion** | Discord — we read every message |
| 🐦 **Tweet about Setu** | Tag [@Hetu_Protocol](https://twitter.com/Hetu_Protocol) |

---

## 📈 Stargazers Over Time

<a href="https://www.star-history.com/#AdvaitaLabs/Setu&Date">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=AdvaitaLabs/Setu&type=Date&theme=dark" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=AdvaitaLabs/Setu&type=Date" />
   <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=AdvaitaLabs/Setu&type=Date" />
 </picture>
</a>

---

## 👥 Contributors

<a href="https://github.com/AdvaitaLabs/Setu/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=AdvaitaLabs/Setu" alt="Contributors" />
</a>

<sub>Made with <a href="https://contrib.rocks">contrib.rocks</a></sub>

---

## 📄 License

This project is licensed under the **Apache License 2.0** — see [LICENSE](LICENSE) for details.

---

## 📖 Citation

If Setu helps your research, please cite it:

```bibtex
@software{setu2026,
  title  = {Setu: A Causally-Driven Distributed Ledger for AI Agent Economies},
  author = {{Advaita Labs}},
  year   = {2026},
  url    = {https://github.com/AdvaitaLabs/Setu}
}
```

---

<div align="center">

<pre>
 ┌─┐┌─┐┌┬┐┬ ┬
 └─┐├┤  │ │ │
 └─┘└─┘ ┴ └─┘
</pre>

<sub><b>Built for the AI-native economy by <a href="https://github.com/AdvaitaLabs">Advaita Labs</a></b></sub>

<br/><br/>

<a href="https://hetu.org">hetu.org</a> &nbsp;·&nbsp; <a href="https://twitter.com/Hetu_Protocol">@Hetu_Protocol</a> &nbsp;·&nbsp; <a href="https://discord.gg/hetu">Discord</a> &nbsp;·&nbsp; <a href="https://docs.hetu.org">Docs</a>

<br/><br/>

<sub>⭐ <a href="https://github.com/AdvaitaLabs/Setu/stargazers">Star us on GitHub</a> &nbsp;·&nbsp; 🐦 <a href="https://twitter.com/intent/tweet?text=Just%20found%20Setu%20-%20a%20causally-driven%20ledger%20for%20AI%20agents.%20Check%20it%20out!&url=https://github.com/AdvaitaLabs/Setu">Share on Twitter</a></sub>

</div>
